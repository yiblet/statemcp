//! Shared file contracts.
use schemars::{JsonSchema, Schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

fn object_root(schema: &mut Schema) {
    schema.insert("type".into(), Value::String("object".into()));
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsReadInput {
    pub namespace: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsStatInput {
    pub namespace: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsListInput {
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileTextInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
    pub path: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileBase64Input {
    pub base64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(transform = object_root)]
pub enum FsWriteInput {
    Text(FileTextInput),
    Binary(FileBase64Input),
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct FsAppendInput(pub FsWriteInput);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsMoveInput {
    pub destination: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsCopyInput {
    pub destination: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsDeleteInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub recursive: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    File,
    Directory,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub kind: FileKind,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileEntries {
    pub entries: Vec<FileEntry>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FileStat {
    File {
        path: String,
        kind: FileKind,
        size: usize,
        hash: String,
    },
    Directory {
        path: String,
        kind: FileKind,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FileContents {
    Text {
        path: String,
        kind: FileKind,
        size: usize,
        text: String,
    },
    Binary {
        path: String,
        kind: FileKind,
        size: usize,
        base64: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileWritten {
    pub path: String,
    pub size: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileMoved {
    pub path: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FilesDeleted {
    pub deleted: usize,
}
