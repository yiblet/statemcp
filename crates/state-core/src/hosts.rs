use crate::{Error, Result, bounded, dispatch::Root, policy::Access};
use serde_json::{Map, Value, json};

impl Root {
    pub(crate) fn host(
        &mut self,
        name: &str,
        positional: Vec<Value>,
        keywords: Map<String, Value>,
        access: &Access,
        namespace: Option<&str>,
    ) -> Result<Value> {
        let result = self.host_inner(name, positional, keywords, access, namespace);
        if let Err(error) = &result {
            self.failure.get_or_insert_with(|| error.clone());
        }
        result
    }
    fn host_inner(
        &mut self,
        name: &str,
        positional: Vec<Value>,
        keywords: Map<String, Value>,
        access: &Access,
        namespace: Option<&str>,
    ) -> Result<Value> {
        self.check()?;
        bounded(
            &json!({"args":positional,"kwargs":keywords}),
            self.limits.runtime.max_output_bytes,
        )?;
        let (tool, arguments, text_only) = match name {
            "mcp" => {
                let args = bind(positional, keywords, &["name", "arguments"], 1)?;
                let tool = args["name"]
                    .as_str()
                    .ok_or_else(|| Error::invalid("mcp name must be a string"))?
                    .to_owned();
                (
                    tool,
                    args.get("arguments").cloned().unwrap_or_else(|| json!({})),
                    false,
                )
            }
            "call" => {
                let mut args = bind(
                    positional,
                    keywords,
                    &["namespace", "function", "arguments"],
                    2,
                )?;
                args.entry("arguments").or_insert_with(|| json!({}));
                ("state_call".into(), json!(args), false)
            }
            "db_query" | "db_execute" | "db_inspect" => {
                let inspect = name == "db_inspect";
                let mut args = bind(
                    positional,
                    keywords,
                    if inspect {
                        &["database"]
                    } else {
                        &["database", "sql", "params"]
                    },
                    if inspect { 1 } else { 2 },
                )?;
                args.insert(
                    "namespace".into(),
                    json!(namespace.ok_or_else(|| Error::invalid(
                        "database helper requires a default namespace"
                    ))?),
                );
                args.insert(
                    "action".into(),
                    json!(match name {
                        "db_query" => "query",
                        "db_execute" => "execute",
                        _ => "inspect",
                    }),
                );
                ("state_db".into(), json!(args), false)
            }
            "read_text" | "write_text" => {
                let read = name == "read_text";
                let mut args = bind(
                    positional,
                    keywords,
                    if read { &["path"] } else { &["path", "text"] },
                    if read { 1 } else { 2 },
                )?;
                args.insert(
                    "namespace".into(),
                    json!(namespace.ok_or_else(|| Error::invalid(
                        "file helper requires a default namespace"
                    ))?),
                );
                args.insert("action".into(), json!(if read { "read" } else { "write" }));
                ("state_fs".into(), json!(args), read)
            }
            _ => return Err(Error::invalid("unknown host function")),
        };
        let result = self.dispatch(&tool, arguments, access, false)?;
        if text_only {
            return result
                .get("text")
                .cloned()
                .ok_or_else(|| Error::invalid("file is not UTF-8 text"));
        }
        Ok(result)
    }
}

fn bind(
    positional: Vec<Value>,
    keywords: Map<String, Value>,
    names: &[&str],
    required: usize,
) -> Result<Map<String, Value>> {
    if positional.len() > names.len() {
        return Err(Error::invalid("too many host arguments"));
    }
    let mut args: Map<_, _> = positional
        .into_iter()
        .zip(names)
        .map(|(value, name)| ((*name).into(), value))
        .collect();
    for (name, value) in keywords {
        if !names.contains(&name.as_str()) {
            return Err(Error::invalid(format!("unknown host argument {name}")));
        }
        if args.insert(name, value).is_some() {
            return Err(Error::invalid("host argument supplied more than once"));
        }
    }
    for name in &names[..required] {
        if !args.contains_key(*name) {
            return Err(Error::invalid(format!("missing host argument {name}")));
        }
    }
    Ok(args)
}
