//! Embeds the browser application in the extension, so that serving it needs no network and no files on disk.
//!
//! `WEBUI_ASSETS` names the directory that holds the built application (see the Dockerfile's `build-web` stage).
//! Without it the extension still compiles, for editors and `cargo check`, and answers page requests with a notice.

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=WEBUI_ASSETS");

    let mut entries = Vec::new();
    match env::var("WEBUI_ASSETS") {
        Ok(directory) => {
            println!("cargo:rerun-if-changed={directory}");

            let mut files: Vec<PathBuf> = fs::read_dir(&directory)
                .unwrap_or_else(|error| panic!("WEBUI_ASSETS={directory}: {error}"))
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.is_file())
                .collect();
            files.sort();

            for file in files {
                println!("cargo:rerun-if-changed={}", file.display());
                let name = file.file_name().unwrap().to_str().unwrap().to_string();
                let path = fs::canonicalize(&file).unwrap();
                entries.push(format!("    ({name:?}, include_bytes!({path:?})),"));
            }
        }
        Err(_) => println!("cargo:warning=WEBUI_ASSETS is not set: webui is built without its browser application"),
    }

    let source = format!(
        "/// The files of the browser application: name and content.\npub static ASSETS: &[(&str, &[u8])] = &[\n{}\n];\n",
        entries.join("\n")
    );
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("assets.rs");
    fs::write(out, source).unwrap();
}
