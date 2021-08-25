use std::collections::BTreeMap;
use std::fmt::Debug;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::{fs, u32};

use bincode;
use byteorder::{LittleEndian, WriteBytesExt};
use glob::glob;
use yada::builder::DoubleArrayBuilder;

use lindera_core::character_definition::{CharacterDefinitions, CharacterDefinitionsBuilder};
use lindera_core::dictionary_builder::DictionaryBuilder;
use lindera_core::error::LinderaErrorKind;
use lindera_core::file_util::read_utf8_file;
use lindera_core::unknown_dictionary::parse_unk;
use lindera_core::user_dictionary::UserDictionary;
use lindera_core::word_entry::{WordEntry, WordId};
use lindera_core::LinderaResult;

#[derive(Debug)]
pub struct CsvRow<'a> {
    surface_form: &'a str,
    left_id: u32,
    right_id: u32,
    word_cost: i32,

    pos_level1: &'a str,
    pos_level2: &'a str,
    pos_level3: &'a str,
    pos_level4: &'a str,

    pub conjugation_type: &'a str,
    pub conjugate_form: &'a str,

    pub base_form: &'a str,
    pub reading: &'a str,
    pronunciation: &'a str,
}

impl<'a> CsvRow<'a> {
    fn from_line(line: &'a str) -> LinderaResult<CsvRow<'a>> {
        let fields: Vec<_> = line.split(',').collect();
        Ok(CsvRow {
            surface_form: &fields[0],
            left_id: u32::from_str(fields[1]).map_err(|_err| {
                LinderaErrorKind::Parse.with_error(anyhow::anyhow!("failed to parse left_id"))
            })?,
            right_id: u32::from_str(fields[2]).map_err(|_err| {
                LinderaErrorKind::Parse.with_error(anyhow::anyhow!("failed to parse right_id"))
            })?,
            word_cost: i32::from_str(fields[3]).map_err(|_err| {
                LinderaErrorKind::Parse.with_error(anyhow::anyhow!("failed to parse word_cost"))
            })?,

            pos_level1: fields[4],
            pos_level2: fields[5],
            pos_level3: fields[6],
            pos_level4: fields[7],

            conjugation_type: fields[8],
            conjugate_form: fields[9],

            base_form: fields[10],
            reading: fields[11],
            pronunciation: fields[12],
        })
    }
}

const SKIP_WORDS: [&'static str; 2] = ["カブシキガイシャ", "タカラヅカカゲキダンキセイ"];

pub struct IpadicNeologdBuilder {}

impl IpadicNeologdBuilder {
    pub fn new() -> Self {
        IpadicNeologdBuilder {}
    }
}

impl DictionaryBuilder for IpadicNeologdBuilder {
    fn build_dictionary(&self, input_dir: &Path, output_dir: &Path) -> LinderaResult<()> {
        fs::create_dir_all(&output_dir)
            .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?;

        let chardef = self.build_chardef(input_dir, output_dir)?;
        self.build_unk(input_dir, &chardef, output_dir)?;
        self.build_dict(input_dir, output_dir)?;
        self.build_cost_matrix(input_dir, output_dir)?;

        Ok(())
    }

    fn build_chardef(
        &self,
        input_dir: &Path,
        output_dir: &Path,
    ) -> LinderaResult<CharacterDefinitions> {
        println!("BUILD CHARDEF");
        let char_def_path = input_dir.join("char.def");
        let char_def = read_utf8_file(&char_def_path)?;
        let mut char_definitions_builder = CharacterDefinitionsBuilder::default();
        char_definitions_builder.parse(&char_def)?;
        let char_definitions = char_definitions_builder.build();

        let wtr_chardef_path = output_dir.join(Path::new("char_def.bin"));
        let mut wtr_chardef = io::BufWriter::new(
            File::create(wtr_chardef_path)
                .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?,
        );
        bincode::serialize_into(&mut wtr_chardef, &char_definitions)
            .map_err(|err| LinderaErrorKind::Serialize.with_error(anyhow::anyhow!(err)))?;
        wtr_chardef
            .flush()
            .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?;

        Ok(char_definitions)
    }

    fn build_unk(
        &self,
        input_dir: &Path,
        chardef: &CharacterDefinitions,
        output_dir: &Path,
    ) -> LinderaResult<()> {
        println!("BUILD UNK");
        let unk_data_path = input_dir.join("unk.def");
        let unk_data = read_utf8_file(&unk_data_path)?;
        let unknown_dictionary = parse_unk(chardef.categories(), &unk_data)?;
        let wtr_unk_path = output_dir.join(Path::new("unk.bin"));
        println!("creating {:?}", wtr_unk_path);
        let mut wtr_unk = io::BufWriter::new(
            File::create(wtr_unk_path)
                .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?,
        );
        bincode::serialize_into(&mut wtr_unk, &unknown_dictionary)
            .map_err(|err| LinderaErrorKind::Serialize.with_error(anyhow::anyhow!(err)))?;
        wtr_unk
            .flush()
            .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?;

        Ok(())
    }

    fn build_dict(&self, input_dir: &Path, output_dir: &Path) -> LinderaResult<()> {
        println!("BUILD DICT");

        let pattern = if let Some(path) = input_dir.to_str() {
            format!("{}/*.csv", path)
        } else {
            return Err(
                LinderaErrorKind::Io.with_error(anyhow::anyhow!("Failed to convert path to &str."))
            );
        };

        let mut filenames: Vec<PathBuf> = Vec::new();
        for entry in
            glob(&pattern).map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?
        {
            match entry {
                Ok(path) => {
                    if let Some(filename) = path.file_name() {
                        filenames.push(Path::new(input_dir).join(filename));
                    } else {
                        return Err(LinderaErrorKind::Io
                            .with_error(anyhow::anyhow!("failed to get filename")));
                    }
                }
                Err(err) => return Err(LinderaErrorKind::Content.with_error(anyhow::anyhow!(err))),
            }
        }

        let files_data: Vec<String> = filenames
            .iter()
            .map(|filename| read_utf8_file(filename))
            .collect::<LinderaResult<Vec<String>>>()?;

        let lines: Vec<String> = files_data
            .iter()
            .flat_map(|file_data: &String| file_data.lines().map(|line| line.to_string()))
            .map(|line| {
                line.chars()
                    .map(|c| {
                        match c {
                            '―' => '—', // yeah for EUC_JP and ambiguous unicode 8012 vs 8013
                            '～' => '〜', // same bullshit as above between for 12316 vs 65374
                            _ => c,
                        }
                    })
                    .collect::<String>()
            })
            .collect();

        let mut rows: Vec<CsvRow> = lines
            .iter()
            .map(|line| CsvRow::from_line(line))
            .collect::<Result<_, _>>()
            .map_err(|err| LinderaErrorKind::Parse.with_error(anyhow::anyhow!(err)))?;
        println!("sorting entries");
        rows.sort_by_key(|row| row.surface_form.clone());

        let wtr_da_path = output_dir.join(Path::new("dict.da"));
        println!("creating {:?}", wtr_da_path);
        let mut wtr_da = io::BufWriter::new(
            File::create(wtr_da_path)
                .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?,
        );

        let wtr_vals_path = output_dir.join(Path::new("dict.vals"));
        println!("creating {:?}", wtr_vals_path);
        let mut wtr_vals = io::BufWriter::new(
            File::create(wtr_vals_path)
                .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?,
        );

        let mut word_entry_map: BTreeMap<String, Vec<WordEntry>> = BTreeMap::new();

        for (row_id, row) in rows.iter().enumerate() {
            if !SKIP_WORDS.contains(&row.surface_form) {
                word_entry_map
                    .entry(row.surface_form.to_string())
                    .or_insert_with(Vec::new)
                    .push(WordEntry {
                        word_id: WordId(row_id as u32, true),
                        word_cost: row.word_cost as i16,
                        cost_id: row.left_id as u16,
                    });
            }
        }

        let wtr_words_path = output_dir.join(Path::new("dict.words"));
        println!("creating {:?}", wtr_words_path);
        let mut wtr_words = io::BufWriter::new(
            File::create(wtr_words_path)
                .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?,
        );

        let wtr_words_idx_path = output_dir.join(Path::new("dict.wordsidx"));
        println!("creating {:?}", wtr_words_idx_path);
        let mut wtr_words_idx = io::BufWriter::new(
            File::create(wtr_words_idx_path)
                .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?,
        );

        let mut words_buffer = Vec::new();
        for row in rows.iter() {
            let word = vec![
                row.pos_level1.to_string(),
                row.pos_level2.to_string(),
                row.pos_level3.to_string(),
                row.pos_level4.to_string(),
                row.conjugation_type.to_string(),
                row.conjugate_form.to_string(),
                row.base_form.to_string(),
                row.reading.to_string(),
                row.pronunciation.to_string(),
            ];
            let offset = words_buffer.len();
            wtr_words_idx
                .write_u32::<LittleEndian>(offset as u32)
                .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?;
            bincode::serialize_into(&mut words_buffer, &word)
                .map_err(|err| LinderaErrorKind::Serialize.with_error(anyhow::anyhow!(err)))?;
        }

        wtr_words
            .write_all(&words_buffer[..])
            .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?;
        wtr_words
            .flush()
            .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?;
        wtr_words_idx
            .flush()
            .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?;

        let mut id = 0u32;

        println!("building da");
        let mut keyset: Vec<(&[u8], u32)> = vec![];
        let mut lastlen = 0;
        for (key, word_entries) in &word_entry_map {
            let len = word_entries.len() as u32;
            assert!(
                len < (1 << 5),
                "{} is {} length. Too long. [{}]",
                key,
                len,
                (1 << 5)
            );
            let val = (id << 5) | len;
            keyset.push((key.as_bytes(), val));
            id += len;
            lastlen += len;
        }

        let da_bytes = DoubleArrayBuilder::build(&keyset).ok_or_else(|| {
            LinderaErrorKind::Io.with_error(anyhow::anyhow!("DoubleArray build error."))
        })?;

        wtr_da
            .write_all(&da_bytes[..])
            .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?;
        println!("Last len is {}", lastlen);

        println!("building values");
        for word_entries in word_entry_map.values() {
            for word_entry in word_entries {
                word_entry
                    .serialize(&mut wtr_vals)
                    .map_err(|err| LinderaErrorKind::Serialize.with_error(anyhow::anyhow!(err)))?;
            }
        }
        wtr_vals
            .flush()
            .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?;

        Ok(())
    }

    fn build_cost_matrix(&self, input_dir: &Path, output_dir: &Path) -> LinderaResult<()> {
        println!("BUILD COST MATRIX");
        let matrix_data_path = input_dir.join("matrix.def");
        let matrix_data = read_utf8_file(&matrix_data_path)?;
        let mut lines = Vec::new();
        for line in matrix_data.lines() {
            let fields: Vec<i32> = line
                .split_whitespace()
                .map(i32::from_str)
                .collect::<Result<_, _>>()
                .map_err(|err| LinderaErrorKind::Parse.with_error(anyhow::anyhow!(err)))?;
            lines.push(fields);
        }
        let mut lines_it = lines.into_iter();
        let header = lines_it.next().ok_or_else(|| {
            LinderaErrorKind::Content.with_error(anyhow::anyhow!("unknown error"))
        })?;
        let forward_size = header[0] as u32;
        let backward_size = header[1] as u32;
        let len = 2 + (forward_size * backward_size) as usize;
        let mut costs = vec![i16::max_value(); len];
        costs[0] = forward_size as i16;
        costs[1] = backward_size as i16;
        for fields in lines_it {
            let forward_id = fields[0] as u32;
            let backward_id = fields[1] as u32;
            let cost = fields[2] as u16;
            costs[2 + (backward_id + forward_id * backward_size) as usize] = cost as i16;
        }

        let wtr_matrix_mtx_path = output_dir.join(Path::new("matrix.mtx"));
        let mut wtr_matrix_mtx = io::BufWriter::new(
            File::create(wtr_matrix_mtx_path)
                .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?,
        );
        for cost in costs {
            wtr_matrix_mtx
                .write_i16::<LittleEndian>(cost)
                .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?;
        }
        wtr_matrix_mtx
            .flush()
            .map_err(|err| LinderaErrorKind::Io.with_error(anyhow::anyhow!(err)))?;

        Ok(())
    }

    fn build_user_dict(&self, input_file: &Path) -> LinderaResult<UserDictionary> {
        unimplemented!();
    }
}
