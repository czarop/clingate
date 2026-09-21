//! A browse button beside a path field.
//!
//! Every file this program reads or writes is named by a path in a text box -
//! the gating file, the rules sidecar, the FCS folder, the exported document.
//! The box stays, and this sits next to it.
//!
//! Both, rather than one or the other. A dialog is the easy way to find a file
//! you can see; a pasted path is the easy way to reach one you cannot - a
//! mounted share, a directory behind a symlink, a name copied out of a ticket.
//! And the dialog is an XDG desktop portal, a D-Bus service separate from this
//! application, so on a machine that is not running one the box is the only way
//! in at all.
//!
//! The backend is not a choice this program gets to make: rfd's build script
//! refuses `gtk3` and `xdg-portal` together, and dioxus-desktop requires the
//! latter, so the portal it is - even though this application is itself a GTK
//! window whose own toolkit would always have been available.

use dioxus::prelude::*;

use crate::components::toast::{use_toast, warn};

/// Which dialog to open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pick {
    /// Choose a file that exists, to read.
    OpenFile,
    /// Name a file to write, which need not exist yet.
    ///
    /// A separate dialog from `OpenFile`, not a flag on it: an open dialog
    /// cannot name a file that is not there, and most of the paths this
    /// program writes to are files that are not there yet.
    SaveFile,
    /// Choose a directory.
    Folder,
}

#[component]
pub fn PickPath(
    /// The field to fill in. Left alone if the dialog is cancelled.
    path: Signal<String>,
    mode: Pick,
    /// What to call the file type in the dialog's filter list.
    #[props(default = String::new())]
    label: String,
    /// Extensions the filter offers, without dots. Empty offers no filter,
    /// which is right for a folder.
    #[props(default = Vec::new())]
    extensions: Vec<String>,
    #[props(default = false)] disabled: bool,
) -> Element {
    let mut path = path;
    let toasts = use_toast();

    let browse = move |_| {
        let current = path();
        let label = label.clone();
        let extensions = extensions.clone();
        spawn(async move {
            let opened = std::time::Instant::now();
            let current = std::path::PathBuf::from(current.trim());
            let mut dialog = rfd::AsyncFileDialog::new();
            if !extensions.is_empty() {
                let refs: Vec<&str> = extensions.iter().map(String::as_str).collect();
                dialog = dialog
                    .add_filter(&label, &refs)
                    .add_filter("Every file", &["*"]);
            }
            // Open where the box already points, so the dialog starts beside
            // the last file rather than in the home directory.
            let start = match mode {
                Pick::Folder => Some(current.clone()),
                _ => current.parent().map(|p| p.to_path_buf()),
            };
            if let Some(start) = start.filter(|p| p.is_dir()) {
                dialog = dialog.set_directory(start);
            }
            if mode == Pick::SaveFile
                && let Some(name) = current.file_name().and_then(|n| n.to_str())
            {
                dialog = dialog.set_file_name(name);
            }

            let chosen = match mode {
                Pick::OpenFile => dialog.pick_file().await,
                Pick::SaveFile => dialog.save_file().await,
                Pick::Folder => dialog.pick_folder().await,
            };

            // `None` is the person pressing Cancel, which is not a failure and
            // should leave what they had typed alone.
            if let Some(handle) = chosen {
                path.set(handle.path().display().to_string());
                return;
            }
            // Unless it came back faster than anyone could have pressed Cancel.
            // Where no portal is running rfd reports exactly what it reports
            // for Cancel - nothing - so the two are told apart by how long it
            // took. Saying so beats a button that appears to do nothing, and
            // the box beside it still takes a typed or pasted path.
            if opened.elapsed() < std::time::Duration::from_millis(300) {
                warn(
                    &toasts,
                    "Could not open the file dialog - type or paste the path instead",
                );
            }
        });
    };

    rsx! {
        button {
            class: "path-picker",
            disabled,
            title: match mode {
                Pick::OpenFile => "Choose a file",
                Pick::SaveFile => "Choose where to write",
                Pick::Folder => "Choose a folder",
            },
            onclick: browse,
            "..."
        }
    }
}
