use anyhow::anyhow;
use flow_fcs::keyword::StringableKeyword;
use flow_fcs::parameter::ParameterBuilder;
use flow_fcs::{Header, Metadata, Parameter, ParameterMap, TransformType};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use thiserror::Error;

use anyhow::Result;

#[derive(Error, Debug)]
pub enum FileError {
    #[error("Invalid directory: {dir:?})")]
    InvalidDirectory { dir: String },
}

// #[derive(PartialEq, Clone)]
// pub struct FcsSampleStub {
//     pub name: String,
//     pub full_path: PathBuf,
// }

// impl std::fmt::Display for FcsSampleStub {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         write!(f, "{}", self.name)
//     }
// }

/// The FCS files a workspace holds.
///
/// A list of files rather than a folder. They can come from sub-folders of
/// the workspace, and more can be added from anywhere, so there is no one
/// directory that contains them all - nothing may rebuild a file's path from
/// a folder and its name.
#[derive(PartialEq, Clone, Default)]
pub struct FcsFiles {
    /// The workspace folder, which names inside the program are taken
    /// relative to. See [`crate::workspace::program_name`].
    root: Option<PathBuf>,
    file_list: Vec<FcsSampleStub>,
    /// Files that were asked for and could not be used, and why - kept so the
    /// workspace can show them, rather than printed to a console nobody reads.
    unread: Vec<Unread>,
}

/// A file that was asked for and could not be used.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct Unread {
    pub path: PathBuf,
    pub reason: String,
}

impl FcsFiles {
    /// Open `paths` as the files of a workspace rooted at `root`.
    ///
    /// Never fails as a whole. One unreadable file is one entry in
    /// [`FcsFiles::unread`], not a workspace that will not open.
    pub fn open(root: Option<&Path>, paths: &[PathBuf]) -> Self {
        let mut files = Self {
            root: root.map(Path::to_path_buf),
            ..Self::default()
        };
        files.add(paths);
        files
    }

    /// Add files, skipping any already here.
    ///
    /// A file whose name inside the program is already taken is refused
    /// rather than loaded: the metadata matches files by that name, so the
    /// second would silently be given the first one's sample.
    pub fn add(&mut self, paths: &[PathBuf]) {
        for path in paths {
            if self.file_list.iter().any(|f| f.filepath == *path) {
                continue;
            }
            // Asked for again: whatever was wrong last time is re-tried, not
            // reported twice.
            self.unread.retain(|u| u.path != *path);

            let name = crate::workspace::program_name(self.root.as_deref(), path);
            if let Some(taken) = self.file_list.iter().find(|f| *f.name == *name) {
                self.unread.push(Unread {
                    path: path.clone(),
                    reason: format!(
                        "its name in the program, {name}, is already used by {}. Files are \
                         matched to their metadata by that name, so this one would be given \
                         the other's sample",
                        taken.filepath.display()
                    ),
                });
                continue;
            }
            match FcsSampleStub::open(&path.to_string_lossy()) {
                Ok(stub) => self.file_list.push(stub.named(name)),
                Err(e) => self.unread.push(Unread {
                    path: path.clone(),
                    reason: e.to_string(),
                }),
            }
        }
        // By name, so the list reads the same whatever order the disk or the
        // dialog handed the files over in.
        self.file_list.sort_by(|a, b| a.name.cmp(&b.name));
        self.unread.sort_by(|a, b| a.path.cmp(&b.path));
    }

    /// Take a file out, whether it loaded or not. Says whether it was here.
    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.file_list.len() + self.unread.len();
        self.file_list.retain(|f| f.filepath != path);
        self.unread.retain(|u| u.path != path);
        before != self.file_list.len() + self.unread.len()
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub fn file_list(&self) -> &[FcsSampleStub] {
        &self.file_list
    }

    pub fn unread(&self) -> &[Unread] {
        &self.unread
    }

    /// Every file that loaded, for remembering the workspace.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.file_list.iter().map(|f| f.filepath.clone()).collect()
    }

    /// The names the sample list shows: each file's name in the program.
    pub fn get_file_names(&self) -> Vec<String> {
        self.file_list.iter().map(|f| f.name.to_string()).collect()
    }

    pub fn sample_count(&self) -> usize {
        self.file_list.len()
    }
}

#[derive(Debug, Clone)]
pub struct FcsSampleStub {
    /// The header segment of the fcs file, including the version, and byte offsets to the text, data, and analysis segments
    pub header: Header,
    /// The metadata segment of the fcs file, including the delimiter, and a hashmap of keyword/value pairs
    pub metadata: Metadata,
    /// A hashmap of the parameter names and their associated metadata
    pub parameters: ParameterMap,

    pub filepath: PathBuf,

    /// What the program calls this file, and what the metadata is searched
    /// for. Its own file name unless it came from a sub-folder of the
    /// workspace - see [`crate::workspace::program_name`].
    pub name: std::sync::Arc<str>,
}

impl PartialEq for FcsSampleStub {
    /// The same acquisition, by `$GUID` where both files carry one.
    ///
    /// Falls back to the path rather than panicking. This used to `expect` a
    /// GUID on both sides, which took the app down for a file without one.
    ///
    /// Every opened file has one: its own, or - for a file without - a random
    /// one given when it is opened. So two copies of one acquisition are equal
    /// wherever they are, and two opens of a file without a `$GUID` are not.
    /// flow_fcs used to replace every file's own `$GUID` with a random one
    /// (B-FCS-1), so until it was fixed no two files ever compared equal.
    fn eq(&self, other: &Self) -> bool {
        match (self.get_guid(), other.get_guid()) {
            (Ok(a), Ok(b)) => a == b,
            _ => self.filepath == other.filepath,
        }
    }
}

impl FcsSampleStub {
    pub fn new() -> Result<Self> {
        Ok(Self {
            header: Header::new(),
            metadata: Metadata::new(),
            parameters: ParameterMap::default(),
            filepath: PathBuf::new(),
            name: std::sync::Arc::from(""),
        })
    }

    /// Read a file's header and keywords, without its events.
    ///
    /// Every step returns an error rather than panicking. This used to
    /// `expect` each one, so a single truncated, corrupt or mislabelled file
    /// in a folder took the whole application down - and with files now added
    /// one at a time from a dialog, "one bad file" stops being unusual.
    pub fn open(path: &str) -> Result<Self> {
        let file_access = flow_fcs::file::AccessWrapper::new(path)
            .map_err(|e| anyhow!("could not open it: {e}"))?;

        Self::validate_fcs_extension(&file_access.path)?;

        // flow_fcs refuses a file too short for a header, or whose TEXT
        // segment is not inside it. It used to slice by the offsets without
        // checking them, and this checked them first; now it answers itself.
        let header = Header::from_mmap(&file_access.mmap)
            .map_err(|e| anyhow!("its header is not an FCS header: {e}"))?;
        let mut metadata = Metadata::from_mmap(&file_access.mmap, &header);

        metadata
            .validate_text_segment_keywords(&header)
            .map_err(|e| anyhow!("its keywords are incomplete: {e}"))?;
        metadata.validate_guid();

        // The events are not read here, but whether they can be is: where the
        // file places them, and that they are all there. The same check
        // `Fcs::open` makes before reading them, so a file the workspace
        // accepts is one a plot or a rules run can read (was B-FCS-2).
        flow_fcs::Fcs::locate_events(&header, &metadata, file_access.mmap.len())
            .map_err(|e| anyhow!("its events cannot be read: {e}"))?;

        let parameters = Self::generate_parameter_map(&metadata)
            .map_err(|e| anyhow!("its parameters could not be read: {e}"))?;

        let filepath = PathBuf::from(path);
        let name = std::sync::Arc::from(
            filepath
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
                .as_str(),
        );
        Ok(Self {
            parameters,
            header,
            metadata,
            filepath,
            name,
        })
    }

    /// The same file under the name the program will know it by.
    pub fn named(mut self, name: impl Into<std::sync::Arc<str>>) -> Self {
        self.name = name.into();
        self
    }

    /// The name the program knows this file by.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Validates that the file extension is `.fcs`
    /// # Errors
    /// Will return `Err` if the file extension is not `.fcs`
    fn validate_fcs_extension(path: &Path) -> Result<()> {
        let extension = path
            .extension()
            .ok_or_else(|| anyhow!("File has no extension"))?
            .to_str()
            .ok_or_else(|| anyhow!("File extension is not valid UTF-8"))?;

        if extension.to_lowercase() != "fcs" {
            return Err(anyhow!("Invalid file extension: {}", extension));
        }

        Ok(())
    }

    pub fn get_filepath(&self) -> &Path {
        &self.filepath
    }

    // pub fn get_sample_name(&self) -> Result<&str> {
    //     if let Ok(flow_fcs::keyword::StringKeyword::FIL(d)) = self.metadata.get_string_keyword("$FIL"){
    //         return Ok(d)
    //     } else {
    //         return Err(anyhow!("could not find sample name in metadata"))
    //     }
    // }

    pub fn find_parameter(&self, parameter_name: &str) -> Result<&Parameter> {
        // Try exact match first (fast path)
        if let Some(param) = self.parameters.get(parameter_name) {
            return Ok(param);
        }

        // Case-insensitive fallback: search through parameter map
        for (key, param) in self.parameters.iter() {
            if key.eq_ignore_ascii_case(parameter_name) {
                return Ok(param);
            }
        }

        Err(anyhow!("Parameter not found: {parameter_name}"))
    }

    /// Looks for the parameter name as a key in the `parameters` hashmap and returns a mutable reference to it
    /// Performs case-insensitive lookup for parameter names
    /// # Errors
    /// Will return `Err` if the parameter name is not found in the `parameters` hashmap
    pub fn find_mutable_parameter(&mut self, parameter_name: &str) -> Result<&mut Parameter> {
        // Try exact match first (fast path)
        // Note: We need to check if the key exists as Arc<str>, so we iterate to find exact match
        let exact_key = self
            .parameters
            .keys()
            .find(|k| k.as_ref() == parameter_name)
            .cloned();

        if let Some(key) = exact_key {
            return self
                .parameters
                .get_mut(&key)
                .ok_or_else(|| anyhow!("Parameter not found: {parameter_name}"));
        }

        // Case-insensitive fallback: find the key first (clone Arc to avoid borrow issues)
        let matching_key = self
            .parameters
            .keys()
            .find(|key| key.eq_ignore_ascii_case(parameter_name))
            .cloned();

        if let Some(key) = matching_key {
            return self
                .parameters
                .get_mut(&key)
                .ok_or_else(|| anyhow!("Parameter not found: {parameter_name}"));
        }

        Err(anyhow!("Parameter not found: {parameter_name}"))
    }

    /// Creates a new `HashMap` of `Parameter`s
    /// using the `Fcs` file's metadata to find the channel and label names from the `PnN` and `PnS` keywords.
    /// Does NOT store events on the parameter.
    /// # Errors
    /// Will return `Err` if:
    /// - the number of parameters cannot be found in the metadata,
    /// - the parameter name cannot be found in the metadata,
    /// - the parameter cannot be built (using the Builder pattern)
    pub fn generate_parameter_map(metadata: &Metadata) -> Result<ParameterMap> {
        let mut map = ParameterMap::default();
        let number_of_parameters = metadata.get_number_of_parameters()?;
        for parameter_number in 1..=*number_of_parameters {
            let channel_name = metadata.get_parameter_channel_name(parameter_number)?;

            // Use label name or fallback to the parameter name
            let label_name = match metadata.get_parameter_label(parameter_number) {
                Ok(label) => label,
                Err(_) => channel_name,
            };

            let transform = if channel_name.contains("FSC")
                || channel_name.contains("SSC")
                || channel_name.contains("Time")
            {
                TransformType::Linear
            } else {
                TransformType::default()
            };

            // Get excitation wavelength from metadata if available
            let excitation_wavelength = metadata
                .get_parameter_excitation_wavelength(parameter_number)
                .ok()
                .flatten();

            let parameter = ParameterBuilder::default()
                // For the ParameterBuilder, ensure we're using the proper methods
                // that may be defined by the Builder derive macro
                .parameter_number(parameter_number)
                .channel_name(channel_name)
                .label_name(label_name)
                .transform(transform)
                .excitation_wavelength(excitation_wavelength)
                .build()?;

            // Add the parameter events to the hashmap keyed by the parameter name
            map.insert(channel_name.to_string().into(), parameter);
        }

        Ok(map)
    }

    /// Looks for a keyword among the metadata and returns its value as a `&str`
    /// # Errors
    /// Will return `Err` if the `Keyword` is not found in the `metadata` or if the `Keyword` cannot be converted to a `&str`
    pub fn get_keyword_string_value(&self, keyword: &str) -> Result<Cow<'_, str>> {
        // TODO: This should be a match statement
        if let Ok(keyword) = self.metadata.get_string_keyword(keyword) {
            Ok(keyword.get_str())
        } else if let Ok(keyword) = self.metadata.get_integer_keyword(keyword) {
            Ok(keyword.get_str())
        } else if let Ok(keyword) = self.metadata.get_float_keyword(keyword) {
            Ok(keyword.get_str())
        } else if let Ok(keyword) = self.metadata.get_byte_keyword(keyword) {
            Ok(keyword.get_str())
        } else if let Ok(keyword) = self.metadata.get_mixed_keyword(keyword) {
            Ok(keyword.get_str())
        } else {
            Err(anyhow!("Keyword not found: {}", keyword))
        }
    }
    /// A convenience function to return the `GUID` keyword from the `metadata` as a `&str`
    /// # Errors
    /// Will return `Err` if the `GUID` keyword is not found in the `metadata` or if the `GUID` keyword cannot be converted to a `&str`
    pub fn get_guid(&self) -> Result<Cow<'_, str>> {
        Ok(self.metadata.get_string_keyword("$GUID")?.get_str())
    }

    /// Set or update the GUID keyword in the file's metadata
    pub fn set_guid(&mut self, guid: String) {
        self.metadata
            .insert_string_keyword("$GUID".to_string(), guid);
    }

    /// A convenience function to return the `$FIL` keyword from the `metadata` as a `&str`
    /// # Errors
    /// Will return `Err` if the `$FIL` keyword is not found in the `metadata` or if the `$FIL` keyword cannot be converted to a `&str`
    pub fn get_fil_keyword(&self) -> Result<Cow<'_, str>> {
        Ok(self.metadata.get_string_keyword("$FIL")?.get_str())
    }

    /// A convenience function to return the `$TOT` keyword from the `metadata` as a `usize`
    /// # Errors
    /// Will return `Err` if the `$TOT` keyword is not found in the `metadata` or if the `$TOT` keyword cannot be converted to a `usize`
    pub fn get_number_of_events(&self) -> Result<&usize> {
        self.metadata.get_number_of_events()
    }

    /// A convenience function to return the `$PAR` keyword from the `metadata` as a `usize`
    /// # Errors
    /// Will return `Err` if the `$PAR` keyword is not found in the `metadata` or if the `$PAR` keyword cannot be converted to a `usize`
    pub fn get_number_of_parameters(&self) -> Result<&usize> {
        self.metadata.get_number_of_parameters()
    }
}
