use std::{collections::HashMap, path::PathBuf};

use dioxus::prelude::*;
use polars::prelude::*;
use rustc_hash::{FxBuildHasher, FxHashMap};

use crate::gate_editor::gates::gate_store::{FileId, GroupId};

pub type MetaDataParameter = Arc<str>;

#[derive(PartialEq, Clone, Hash, Debug, Eq)]
pub struct MetaDataKey {
    pub parameter: MetaDataParameter,
    pub group: GroupId,
}
// pub type MetaDataFileMap =
//     im::HashMap<MetaDataParameter, FxHashMap<FileId, GroupId>, FxBuildHasher>;
pub type MetaDataFileMap =
    im::HashMap<FileId, FxHashMap<MetaDataParameter, GroupId>, FxBuildHasher>;

pub enum MetaDataOrigin {
    Omiq,
}

#[derive(Store, Clone, Default)]
pub struct MetaDataStore {
    metadata: MetaDataFileMap,
    // map of file names -> gating id's so we can associate the actual files with metadata
    file_name_to_gating_id: HashMap<Arc<str>, FileId, FxBuildHasher>,
    // used for omiq where file id's start with 'f' .. but not in the gating jsons (thanks omiq!)
    gating_id_to_actual_id_override_map: HashMap<FileId, String, FxBuildHasher>,
}

#[store(pub name = MetaDataImplExt)]
impl<Lens> Store<MetaDataStore, Lens> {
    fn set_metadata_from_file(
        &mut self,
        path: PathBuf,
        file_id_column: &str,
        file_name_column: &str,
        metadata_origin: MetaDataOrigin,
    ) -> anyhow::Result<Vec<String>> {
        let parsed = parse_metadata_csv(path, file_id_column, file_name_column, metadata_origin)?;
        let warnings = parsed.warnings();
        self.with_mut(|s| {
            s.metadata = parsed.metadata;
            s.file_name_to_gating_id = parsed.file_name_to_gating_id;
            s.gating_id_to_actual_id_override_map = parsed.gating_id_to_actual_id;
        });

        Ok(warnings)
    }
}

/// What a metadata export yields once parsed, before it reaches the store.
pub struct ParsedMetaData {
    pub metadata: MetaDataFileMap,
    /// Maps the FCS `$FIL` name to the id the gating JSON uses.
    pub file_name_to_gating_id: HashMap<Arc<str>, FileId, FxBuildHasher>,
    /// Maps the gating id back to the id the metadata export used, restoring
    /// Omiq's `F` prefix. The export path needs this to address files the way
    /// Omiq expects.
    pub gating_id_to_actual_id: HashMap<FileId, String, FxBuildHasher>,
    /// Rows that could not be tied to a file, and so were not read.
    pub skipped: Vec<SkippedRow>,
    /// File names on more than one row, in the order they first appear.
    /// Their rows are read, by id, but no file on disk is tied to them by
    /// name.
    pub shared_names: Vec<SharedName>,
}

impl ParsedMetaData {
    /// What a person should be told about the export once it is loaded.
    pub fn warnings(&self) -> Vec<String> {
        [
            skipped_rows_message(&self.skipped),
            shared_names_message(&self.shared_names),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// A file name on more than one row of a metadata export.
///
/// Omiq allows it - two plates' `A1.fcs`, told apart by id - so the rows are
/// kept, under their ids, for the gating file that refers to them by id. But
/// a file on disk is found by its name, and nothing says which row one called
/// `A1.fcs` is, so none is given either row's metadata rather than a guess.
#[derive(Debug, Clone, PartialEq)]
pub struct SharedName {
    pub name: Arc<str>,
    /// Numbered as a spreadsheet numbers them: the header is row 1.
    pub rows: Vec<usize>,
}

impl std::fmt::Display for SharedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let rows: Vec<String> = self.rows.iter().map(usize::to_string).collect();
        let (last, rest) = rows
            .split_last()
            .expect("a shared name is on two rows or more");
        write!(f, "{} (rows {} and {last})", self.name, rest.join(", "))
    }
}

/// A row of a metadata export with something in it but no file id or no file
/// name, so no file it could describe. Such a row is left out and reported.
#[derive(Debug, Clone, PartialEq)]
pub struct SkippedRow {
    /// Numbered as a spreadsheet numbers it: the header is row 1.
    pub row: usize,
    /// The id or file name the row does have, to find it by.
    pub has: Option<String>,
    /// What it is missing: the id column's name, the file name column's, or
    /// both.
    pub missing: String,
}

impl std::fmt::Display for SkippedRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "row {}", self.row)?;
        if let Some(has) = &self.has {
            write!(f, " ({has})")?;
        }
        write!(f, " has no {}", self.missing)
    }
}

/// The first few of `items`, and how many more there are.
fn first_few<T: std::fmt::Display>(items: &[T]) -> String {
    const NAMED: usize = 5;
    let mut named: Vec<String> = items.iter().take(NAMED).map(T::to_string).collect();
    if items.len() > NAMED {
        named.push(format!("and {} more", items.len() - NAMED));
    }
    named.join("; ")
}

/// What to tell a person about rows left out of a metadata export, if any.
/// The first few are named; the rest are counted.
pub fn skipped_rows_message(skipped: &[SkippedRow]) -> Option<String> {
    let one = skipped.len() == 1;
    (!skipped.is_empty()).then(|| {
        format!(
            "{} metadata row{} could not be tied to a file and {} left out: {}",
            skipped.len(),
            if one { "" } else { "s" },
            if one { "was" } else { "were" },
            first_few(skipped)
        )
    })
}

/// What to tell a person about file names on more than one row, if any.
pub fn shared_names_message(shared: &[SharedName]) -> Option<String> {
    let one = shared.len() == 1;
    (!shared.is_empty()).then(|| {
        format!(
            "{} file name{} on more than one metadata row, so no file of that name is given metadata: {}",
            shared.len(),
            if one { " is" } else { "s are" },
            first_few(shared)
        )
    })
}

/// Parse a metadata export.
///
/// Split out of the store method so it can be exercised without a Dioxus
/// runtime: everything here is plain parsing over a CSV.
///
/// Each row is read whole - its id, its file name and its metadata together -
/// so a row left out cannot shift another's metadata onto the wrong file.
/// The metadata used to be read by position in the list of rows kept, and
/// every file after a skipped row was given the row before it (B-META-1).
///
/// A row with no id or no file name describes no file: it is left out and
/// reported in [`ParsedMetaData::skipped`]. A row with nothing in it at all,
/// as a spreadsheet can leave at the end, is passed over without a word.
///
/// A file name on more than one row ties no file to any of them - see
/// [`SharedName`] - and is reported in [`ParsedMetaData::shared_names`].
///
/// Refused if two rows have the same id: the id is what the gating file
/// refers to, and there is no telling which row holds that file's metadata.
pub fn parse_metadata_csv(
    path: PathBuf,
    file_id_column: &str,
    file_name_column: &str,
    metadata_origin: MetaDataOrigin,
) -> anyhow::Result<ParsedMetaData> {
    let df = fetch_metadata_from_csv(path)?;

    let raw_ids = df.column(file_id_column)?.str()?;
    let file_names = df.column(file_name_column)?.str()?;
    let metadata_columns = df
        .get_column_names()
        .into_iter()
        .filter(|&name| name != file_id_column && name != file_name_column)
        .map(|name| {
            Ok((
                Arc::<str>::from(name.as_str()),
                df.column(name.as_str())?.str()?,
            ))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let mut master_map: MetaDataFileMap = im::HashMap::with_hasher(FxBuildHasher);
    let mut file_id_overrides: HashMap<FileId, String, FxBuildHasher> =
        HashMap::with_hasher(FxBuildHasher);
    let mut name_to_id: HashMap<Arc<str>, FileId, FxBuildHasher> =
        HashMap::with_hasher(FxBuildHasher);
    // The row each id was first seen on, to name both rows of a duplicate,
    // and every row each file name is on, in the order first seen.
    let mut id_row: HashMap<FileId, usize, FxBuildHasher> = HashMap::with_hasher(FxBuildHasher);
    let mut name_rows: indexmap::IndexMap<Arc<str>, Vec<usize>, FxBuildHasher> =
        indexmap::IndexMap::with_hasher(FxBuildHasher);
    let mut skipped = Vec::new();

    for index in 0..df.height() {
        let row = index + 2;
        let values: Vec<(Arc<str>, &str)> = metadata_columns
            .iter()
            .filter_map(|(column, values)| present(values.get(index)).map(|v| (column.clone(), v)))
            .collect();
        let (raw_id, name) = match (present(raw_ids.get(index)), present(file_names.get(index))) {
            (Some(raw_id), Some(name)) => (raw_id, name),
            (None, None) if values.is_empty() => continue,
            (raw_id, name) => {
                skipped.push(SkippedRow {
                    row,
                    has: raw_id.or(name).map(str::to_string),
                    missing: match (raw_id, name) {
                        (None, None) => format!("{file_id_column} or {file_name_column}"),
                        (None, _) => file_id_column.to_string(),
                        _ => file_name_column.to_string(),
                    },
                });
                continue;
            }
        };

        // Omiq's metadata export prefixes ids with `F`; its gating file does not.
        let gating_id: FileId = match metadata_origin {
            MetaDataOrigin::Omiq if raw_id.starts_with('F') => Arc::from(&raw_id[1..]),
            _ => Arc::from(raw_id),
        };
        let name: Arc<str> = Arc::from(name);

        if let Some(first) = id_row.insert(gating_id.clone(), row) {
            anyhow::bail!(
                "rows {first} and {row} are both for the file with id {raw_id}: each file must have one row, or there is no telling which holds its metadata"
            );
        }

        file_id_overrides.insert(gating_id.clone(), raw_id.to_string());
        name_rows.entry(name.clone()).or_default().push(row);
        name_to_id.insert(name, gating_id.clone());
        master_map.insert(
            gating_id,
            values
                .into_iter()
                .map(|(column, value)| (column, Arc::from(value)))
                .collect(),
        );
    }

    let shared_names: Vec<SharedName> = name_rows
        .into_iter()
        .filter(|(_, rows)| rows.len() > 1)
        .map(|(name, rows)| SharedName { name, rows })
        .collect();
    for shared in &shared_names {
        name_to_id.remove(&shared.name);
    }

    Ok(ParsedMetaData {
        metadata: master_map,
        file_name_to_gating_id: name_to_id,
        gating_id_to_actual_id: file_id_overrides,
        skipped,
        shared_names,
    })
}

/// A cell's value, unless it is empty or only spaces.
fn present(value: Option<&str>) -> Option<&str> {
    value.filter(|v| !v.trim().is_empty())
}

fn fetch_metadata_from_csv(path: PathBuf) -> anyhow::Result<DataFrame> {
    // 1. Read just the first row to get the column names
    let schema_df = CsvReadOptions::default()
        .with_has_header(true)
        .with_n_rows(Some(0)) // Only get headers
        .try_into_reader_with_file_path(Some(path.clone()))?
        .finish()?;

    // 2. Map every column name to DataType::String
    let schema = Schema::from_iter(
        schema_df
            .get_column_names()
            .iter()
            .map(|&name| Field::new(name.clone(), DataType::String)),
    );

    // 3. Read the actual data using our "All-String" schema
    let csv = CsvReadOptions::default()
        .with_has_header(true)
        .with_schema(Some(Arc::new(schema))) // Tell Polars: "Everything is a string"
        .try_into_reader_with_file_path(Some(path))?
        .finish()?;

    Ok(csv)
}
