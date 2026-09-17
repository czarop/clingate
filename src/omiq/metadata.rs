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
    ) -> anyhow::Result<()> {
        let parsed = parse_metadata_csv(path, file_id_column, file_name_column, metadata_origin)?;
        self.with_mut(|s| {
            s.metadata = parsed.metadata;
            s.file_name_to_gating_id = parsed.file_name_to_gating_id;
            s.gating_id_to_actual_id_override_map = parsed.gating_id_to_actual_id;
        });

        Ok(())
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
}

/// Parse a metadata export.
///
/// Split out of the store method so it can be exercised without a Dioxus
/// runtime: everything here is plain parsing over a CSV.
pub fn parse_metadata_csv(
    path: PathBuf,
    file_id_column: &str,
    file_name_column: &str,
    metadata_origin: MetaDataOrigin,
) -> anyhow::Result<ParsedMetaData> {
    let df = fetch_metadata_from_csv(path)?;

    let mut master_map: MetaDataFileMap = im::HashMap::with_hasher(FxBuildHasher);
    let mut file_id_overrides: HashMap<FileId, String, FxBuildHasher> =
        HashMap::with_hasher(FxBuildHasher);

    // Pre-process IDs to handle the 'f' prefix in omiq and build a map
    let raw_ids = df.column(file_id_column)?.str()?;
    let file_names = df.column(file_name_column)?.str()?;
    let len = raw_ids.len();
    let mut processed_ids = Vec::with_capacity(len);
    let mut name_to_id: HashMap<Arc<str>, FileId, FxBuildHasher> =
        HashMap::with_hasher(FxBuildHasher);

    for (raw_id_opt, name_opt) in raw_ids.into_iter().zip(file_names.into_iter()) {
        if let (Some(raw_id), Some(name)) = (raw_id_opt, name_opt) {
            // Handle the Omiq 'f' prefix
            let gating_id: Arc<str> = match metadata_origin {
                MetaDataOrigin::Omiq if raw_id.starts_with('F') => Arc::from(&raw_id[1..]),
                _ => Arc::from(raw_id),
            };

            // Store the Omiq Override (e.g., "123" -> "f123")
            file_id_overrides.insert(gating_id.clone(), raw_id.to_string());

            // Store the Name mapping (e.g., "Sample_A.fcs" -> "123")
            name_to_id.insert(Arc::from(name), gating_id.clone());

            // Keep the ID for the metadata column loops later
            processed_ids.push(gating_id);
        }
    }

    // // Iterate through metadata columns
    // for col_name in df.get_column_names() {
    //     if col_name == file_id_column || col_name == file_name_column {
    //         continue;
    //     }

    //     let mut current_column_map = FxHashMap::default();
    //     let metadata_vals = df.column(col_name)?.str()?;

    //     // Zip the pre-processed IDs with these values
    //     for (actual_id, val_opt) in processed_ids.iter().zip(metadata_vals.into_iter()) {
    //         if let Some(val) = val_opt {
    //             current_column_map.insert(actual_id.clone(), Arc::from(val));
    //         }
    //     }

    //     master_map.insert(Arc::from(col_name.as_str()), current_column_map);
    // }

    // 2. Prepare the metadata columns we actually want to process
    // We filter out the ID and Name columns once to save cycles in the row loop
    let metadata_column_names: Vec<&PlSmallStr> = df
        .get_column_names()
        .into_iter()
        .filter(|&name| name != file_id_column && name != file_name_column)
        .collect();

    // 3. Iterate through every pre-processed FileId
    for (row_idx, actual_id) in processed_ids.iter().enumerate() {
        let mut file_metadata = FxHashMap::default();

        for col_name in &metadata_column_names {
            // Get the value for this specific row in this specific column
            let col = df.column(col_name.as_str())?.str()?;

            if let Some(val) = col.get(row_idx) {
                // Clean the parameter name (drop '$' if needed)
                let param_name = Arc::from(col_name.as_str());
                file_metadata.insert(param_name, Arc::from(val));
            }
        }

        // Insert the complete metadata bundle for this file
        master_map.insert(actual_id.clone(), file_metadata);
    }

    Ok(ParsedMetaData {
        metadata: master_map,
        file_name_to_gating_id: name_to_id,
        gating_id_to_actual_id: file_id_overrides,
    })
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
