//! Stored source references, resolved into runtime declarations only at the catalog boundary.
use crate::{
    Error, ErrorCode, Result,
    identity::hash,
    model::{Database, Manifest},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use state_contract::{
    function::{EndpointSchema, FunctionDeclaration, FunctionSources},
    http::{Handler, Method, Route, RoutePattern, RouteTarget},
};
use std::collections::{BTreeMap, BTreeSet};

/// A canonical SHA-256 digest of immutable bytes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(super) struct ContentHash(String);
impl TryFrom<String> for ContentHash {
    type Error = Error;
    fn try_from(value: String) -> Result<Self> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(corrupt("invalid content digest"));
        }
        Ok(Self(value))
    }
}
impl From<ContentHash> for String {
    fn from(value: ContentHash) -> Self {
        value.0
    }
}
fn corrupt(message: &str) -> Error {
    Error::new(ErrorCode::CorruptStore, message)
}

fn intern(conn: &Connection, bytes: &[u8]) -> Result<ContentHash> {
    let digest = ContentHash(hash(bytes));
    conn.execute(
        "INSERT OR IGNORE INTO objects(hash,bytes) VALUES(?,?)",
        params![digest.0, bytes],
    )?;
    Ok(digest)
}
fn text(conn: &Connection, digest: &ContentHash) -> Result<String> {
    let bytes: Vec<u8> = conn
        .query_row(
            "SELECT bytes FROM objects WHERE hash=?",
            [&digest.0],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| corrupt("pinned source object is missing"))?;
    if hash(&bytes) != digest.0 {
        return Err(corrupt("pinned source digest differs from its bytes"));
    }
    String::from_utf8(bytes).map_err(|_| corrupt("pinned source is not UTF-8"))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceReferences {
    source: ContentHash,
    modules: BTreeMap<String, ContentHash>,
}
impl SourceReferences {
    fn capture(
        conn: &Connection,
        source: &str,
        modules: &BTreeMap<String, String>,
    ) -> Result<Self> {
        Ok(Self {
            source: intern(conn, source.as_bytes())?,
            modules: modules
                .iter()
                .map(|(path, source)| Ok((path.clone(), intern(conn, source.as_bytes())?)))
                .collect::<Result<_>>()?,
        })
    }
    fn resolve(self, conn: &Connection) -> Result<FunctionSources> {
        Ok(FunctionSources {
            source: text(conn, &self.source)?,
            modules: self
                .modules
                .into_iter()
                .map(|(path, digest)| Ok((path, text(conn, &digest)?)))
                .collect::<Result<_>>()?,
        })
    }
    fn add_references(&self, counts: &mut BTreeMap<ContentHash, i64>) -> Result<()> {
        for digest in std::iter::once(&self.source).chain(self.modules.values()) {
            add_reference(counts, digest)?;
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredFunction {
    abi_version: u32,
    description: Option<String>,
    file: String,
    input_schema: Option<EndpointSchema>,
    name: String,
    output_schema: Option<EndpointSchema>,
    sources: SourceReferences,
    symbol: String,
    version: String,
}
impl StoredFunction {
    fn capture(conn: &Connection, function: &FunctionDeclaration) -> Result<Self> {
        let sources = SourceReferences::capture(conn, &function.source, &function.modules)?;
        if sources.source.0 != function.source_hash {
            return Err(corrupt("function source digest is inconsistent"));
        }
        Ok(Self {
            abi_version: function.abi_version,
            description: function.description.clone(),
            file: function.file.clone(),
            input_schema: function.input_schema.clone(),
            name: function.name.clone(),
            output_schema: function.output_schema.clone(),
            sources,
            symbol: function.symbol.clone(),
            version: function.version.clone(),
        })
    }
    fn resolve(self, conn: &Connection) -> Result<FunctionDeclaration> {
        let source_hash = self.sources.source.0.clone();
        let sources = self.sources.resolve(conn)?;
        Ok(FunctionDeclaration {
            abi_version: self.abi_version,
            description: self.description,
            file: self.file,
            input_schema: self.input_schema,
            modules: sources.modules,
            name: self.name,
            output_schema: self.output_schema,
            source: sources.source,
            source_hash,
            symbol: self.symbol,
            version: self.version,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StoredTarget {
    Function {
        file: String,
        symbol: String,
        sources: SourceReferences,
    },
    Static {
        directory: String,
        index: Option<String>,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRoute {
    name: String,
    path: RoutePattern,
    methods: BTreeSet<Method>,
    target: StoredTarget,
}
impl StoredRoute {
    fn capture(conn: &Connection, route: &Route) -> Result<Self> {
        let target = match &route.target {
            RouteTarget::Function(handler) => StoredTarget::Function {
                file: handler.file.clone(),
                symbol: handler.symbol.clone(),
                sources: SourceReferences::capture(
                    conn,
                    &handler.sources.source,
                    &handler.sources.modules,
                )?,
            },
            RouteTarget::Static { directory, index } => StoredTarget::Static {
                directory: directory.clone(),
                index: index.clone(),
            },
        };
        Ok(Self {
            name: route.name.clone(),
            path: route.path.clone(),
            methods: route.methods.clone(),
            target,
        })
    }
    fn resolve(self, conn: &Connection) -> Result<Route> {
        let target = match self.target {
            StoredTarget::Function {
                file,
                symbol,
                sources,
            } => RouteTarget::Function(Handler {
                file,
                symbol,
                sources: sources.resolve(conn)?,
            }),
            StoredTarget::Static { directory, index } => RouteTarget::Static { directory, index },
        };
        Ok(Route {
            name: self.name,
            path: self.path,
            methods: self.methods,
            target,
        })
    }
}

/// Compact storage representation; runtime code never consumes hash references as source text.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredManifest {
    routes: BTreeMap<String, StoredRoute>,
    description: Option<String>,
    files: BTreeMap<String, ContentHash>,
    databases: BTreeMap<String, Database>,
    functions: BTreeMap<String, StoredFunction>,
}
impl StoredManifest {
    pub(super) fn capture(conn: &Connection, manifest: &Manifest) -> Result<Self> {
        Ok(Self {
            routes: manifest
                .routes
                .iter()
                .map(|(name, route)| Ok((name.clone(), StoredRoute::capture(conn, route)?)))
                .collect::<Result<_>>()?,
            description: manifest.description.clone(),
            files: manifest
                .files
                .iter()
                .map(|(path, digest)| Ok((path.clone(), ContentHash::try_from(digest.clone())?)))
                .collect::<Result<_>>()?,
            databases: manifest.databases.clone(),
            functions: manifest
                .functions
                .iter()
                .map(|(name, function)| {
                    Ok((name.clone(), StoredFunction::capture(conn, function)?))
                })
                .collect::<Result<_>>()?,
        })
    }
    pub(super) fn resolve(self, conn: &Connection) -> Result<Manifest> {
        Ok(Manifest {
            routes: self
                .routes
                .into_iter()
                .map(|(name, route)| Ok((name, route.resolve(conn)?)))
                .collect::<Result<_>>()?,
            description: self.description,
            files: self
                .files
                .into_iter()
                .map(|(path, digest)| (path, digest.0))
                .collect(),
            databases: self.databases,
            functions: self
                .functions
                .into_iter()
                .map(|(name, function)| Ok((name, function.resolve(conn)?)))
                .collect::<Result<_>>()?,
        })
    }
    pub(super) fn record_references(&self, conn: &Connection, revision: &str) -> Result<()> {
        let mut counts = BTreeMap::new();
        for digest in self.files.values() {
            add_reference(&mut counts, digest)?;
        }
        for function in self.functions.values() {
            function.sources.add_references(&mut counts)?;
        }
        for route in self.routes.values() {
            if let StoredTarget::Function { sources, .. } = &route.target {
                sources.add_references(&mut counts)?;
            }
        }
        for (digest, count) in counts {
            conn.execute(
                "INSERT INTO revision_objects(revision_id,hash,uses) VALUES(?,?,?)",
                params![revision, digest.0, count],
            )?;
        }
        Ok(())
    }
}
fn add_reference(counts: &mut BTreeMap<ContentHash, i64>, digest: &ContentHash) -> Result<()> {
    let count = counts.entry(digest.clone()).or_default();
    *count = count
        .checked_add(1)
        .ok_or_else(|| corrupt("content reference count overflow"))?;
    Ok(())
}
