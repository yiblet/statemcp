//! Bulk transfers bypass JSON byte envelopes while retaining atomic storage publication.
use crate::{Controller, Error, ErrorCode, Result};
use state_contract::transfer::*;
use std::collections::BTreeSet;

/// A download file set resolved to a stable namespace identity.
#[derive(serde::Serialize)]
pub struct DownloadSelection {
    pub namespace: String,
    pub paths: Vec<TransferPath>,
    pub format: ArchiveFormat,
}
impl Controller {
    /// Resolve upload authority to a stable identity before issuing a transport token.
    pub fn prepare_upload(&self, input: FsUploadInput) -> Result<(String, UploadPaths)> {
        let paths = UploadPaths::parse(&input.files).map_err(|e| Error::new(e.code, e.message))?;
        let ns = self.get_namespace(state_contract::namespace::NamespaceGetInput {
            namespace: input.namespace,
        })?;
        Ok((ns.summary.id, paths))
    }
    /// Expand globs from one namespace snapshot, rejecting unmatched selectors.
    pub fn prepare_download(&self, input: FsDownloadInput) -> Result<DownloadSelection> {
        if input.files.is_empty() || input.files.len() > MAX_TRANSFER_FILES {
            return Err(Error::invalid("files must contain 1..256 selectors"));
        }
        self.transaction(|context| {
            let id = context.namespace(&input.namespace)?;
            let files = context.transaction().file_paths(id.as_str())?;
            let mut selected = BTreeSet::new();
            for selector in &input.files {
                if selector.chars().any(|c| c.is_control() || c == '\\')
                    || selector.len() > 4096
                    || selector.split('/').any(|part| matches!(part, "." | ".."))
                {
                    return Err(Error::invalid("invalid download selector"));
                }
                let selector = if selector.starts_with('/') {
                    selector.clone()
                } else {
                    format!("/{selector}")
                };
                let pattern = glob::Pattern::new(&selector)
                    .map_err(|_| Error::invalid("invalid glob pattern"))?;
                let matches: Vec<_> = files
                    .iter()
                    .filter(|path| {
                        pattern.matches_with(
                            path.as_str(),
                            glob::MatchOptions {
                                case_sensitive: true,
                                require_literal_separator: true,
                                require_literal_leading_dot: false,
                            },
                        )
                    })
                    .cloned()
                    .collect();
                if matches.is_empty() {
                    return Err(Error::new(
                        ErrorCode::NotFound,
                        "download selector matches no files",
                    ));
                }
                selected.extend(matches);
                if selected.len() > MAX_TRANSFER_FILES {
                    return Err(Error::limit("download selects more than 256 files"));
                }
            }
            Ok(DownloadSelection {
                namespace: id.as_str().to_owned(),
                paths: selected.into_iter().collect(),
                format: input.format,
            })
        })
        .map(|(selection, _)| selection)
    }
    /// Import all raw bytes atomically, subject to the transfer-specific total limit.
    /// The iterator may read temporary files synchronously; run on a blocking thread.
    pub fn import_files(
        &self,
        namespace: &str,
        files: impl IntoIterator<Item = Result<(TransferPath, Vec<u8>)>>,
    ) -> Result<Vec<state_contract::file::FileWritten>> {
        let mut tx = self.store().begin()?;
        let mut total = 0usize;
        let mut results = Vec::new();
        for file in files {
            let (path, bytes) = file?;
            total = total
                .checked_add(bytes.len())
                .ok_or_else(|| Error::limit("upload too large"))?;
            if total > MAX_UPLOAD_BYTES || results.len() >= MAX_TRANSFER_FILES {
                return Err(Error::limit("upload exceeds transfer limits"));
            }
            results.push(tx.write_file_bytes(namespace, &path, bytes)?);
        }
        tx.commit()?;
        Ok(results)
    }
    /// Read a complete download from one storage snapshot, one file at a time.
    /// The sink may block for network backpressure; call only on dedicated threads.
    pub fn export_files(
        &self,
        selection: &DownloadSelection,
        mut sink: impl FnMut(&TransferPath, Vec<u8>) -> Result<()>,
    ) -> Result<()> {
        let tx = self.store().begin()?;
        for path in &selection.paths {
            sink(path, tx.file_bytes(&selection.namespace, path.as_str())?)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use state_contract::{execution::*, file::*, namespace::*};
    fn controller() -> Result<Controller> {
        let store = state_store::Store::builder()
            .with_fs(state_fs::MemoryFs::new())
            .open("/state")
            .map_err(state_domain::Error::from)?;
        let controller = Controller::new(
            state_domain::Store::from_storage(store)?,
            crate::Runtime::embedded(),
        );
        controller.create_namespace(NamespaceCreateInput {
            name: "files".into(),
            description: DescriptionChange::Unchanged,
        })?;
        Ok(controller)
    }
    #[test]
    fn bulk_import_larger_than_json_limit_and_glob_export_preserve_bytes() -> Result<()> {
        let controller = controller()?;
        let (namespace, paths) = controller.prepare_upload(FsUploadInput {
            namespace: "files".into(),
            files: vec!["folder/data.bin".into()],
        })?;
        let path = paths
            .paths()
            .first()
            .ok_or_else(|| Error::invalid("missing test path"))?
            .clone();
        let data = vec![0xA5; 9 * 1024 * 1024];
        controller.import_files(&namespace, [Ok((path, data))])?;
        let selection = controller.prepare_download(FsDownloadInput {
            namespace: "files".into(),
            files: vec!["folder/*.bin".into(), "/**/*.bin".into()],
            format: ArchiveFormat::Tar,
        })?;
        assert_eq!(selection.paths.len(), 1);
        controller.export_files(&selection, |path, bytes| {
            assert_eq!(path.archive_name(), "folder/data.bin");
            assert_eq!(bytes.len(), 9 * 1024 * 1024);
            assert!(bytes.iter().all(|byte| *byte == 0xA5));
            Ok(())
        })?;
        assert!(
            controller
                .prepare_download(FsDownloadInput {
                    namespace: "files".into(),
                    files: vec!["none/*".into()],
                    format: ArchiveFormat::Tar
                })
                .is_err()
        );
        assert!(
            controller
                .prepare_download(FsDownloadInput {
                    namespace: "files".into(),
                    files: vec!["../*".into()],
                    format: ArchiveFormat::Tar
                })
                .is_err()
        );
        Ok(())
    }
    #[test]
    fn failed_import_rolls_back_every_file_and_total_limit() -> Result<()> {
        let controller = controller()?;
        let a = TransferPath::parse("a").map_err(|e| Error::new(e.code, e.message))?;
        let b = TransferPath::parse("b").map_err(|e| Error::new(e.code, e.message))?;
        assert!(
            controller
                .import_files(
                    "files",
                    [
                        Ok((a.clone(), vec![0; 6 * 1024 * 1024])),
                        Ok((b, vec![0; 5 * 1024 * 1024]))
                    ]
                )
                .is_err()
        );
        assert!(
            controller
                .read_file(FsReadInput {
                    namespace: "files".into(),
                    path: "/a".into()
                })
                .is_err()
        );
        assert!(
            controller
                .import_files(
                    "files",
                    [
                        Ok((a.clone(), vec![1])),
                        Err(Error::new(ErrorCode::IoError, "staging failed"))
                    ]
                )
                .is_err()
        );
        assert!(
            controller
                .read_file(FsReadInput {
                    namespace: "files".into(),
                    path: "/a".into()
                })
                .is_err()
        );
        Ok(())
    }
    #[test]
    fn config_attributes_available_in_scripts_functions_and_handlers() -> Result<()> {
        let controller = controller()?.with_config(PublicConfig {
            public_url: Some("https://example.com".into()),
            prefix: "/prefix".into(),
        });
        let script = "{'public_url':config().public_url,'prefix':config().prefix}";
        let result = controller.execute_script(
            ExecuteInput {
                script: script.into(),
                inputs: None,
                namespace: None,
                idempotency_key: None,
            },
            None,
        )?;
        assert_eq!(
            result.value,
            serde_json::json!({"public_url":"https://example.com","prefix":"/prefix"})
        );
        controller.write_file(FsWriteInput::Text(FileTextInput {
            namespace: "files".into(),
            path: "/config.py".into(),
            text: format!(
                "def read():\n    return {script}\ndef handle(request):\n    return {script}\n"
            ),
            expected_revision: None,
        }))?;
        controller.declare_function(state_contract::function::FunctionDeclareInput {
            namespace: "files".into(),
            name: "read".into(),
            file: "/config.py".into(),
            symbol: "read".into(),
            input_schema: None,
            output_schema: None,
            description: None,
            expected_revision: None,
            expected_version: None,
        })?;
        let value = controller.call_function(
            CallInput {
                namespace: "files".into(),
                function: "read".into(),
                arguments: None,
                expected_version: None,
                idempotency_key: None,
            },
            None,
        )?;
        assert_eq!(value, result.value);
        use state_contract::http::*;
        controller.publish_api(ApiPublishInput {
            namespace: "files".into(),
            name: "config".into(),
            path: RoutePattern::parse("/config")?,
            methods: Default::default(),
            target: TargetInput::Function {
                file: "/config.py".into(),
                symbol: "handle".into(),
            },
            expected_revision: None,
        })?;
        let response = controller.handle_http(
            "files",
            "/config",
            HttpRequest {
                method: Method::parse("GET")?,
                path: "/prefix/files/config".into(),
                query: "".into(),
                scheme: "http".into(),
                host: "localhost".into(),
                headers: vec![],
                body: vec![],
                view_args: Default::default(),
            },
        )?;
        let value: serde_json::Value =
            serde_json::from_slice(response.body()).map_err(|e| Error::invalid(e.to_string()))?;
        assert_eq!(value, result.value);
        Ok(())
    }
}
