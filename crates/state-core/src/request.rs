//! Parse protocol and host input once; preserve the established contract internally.
use crate::{Operation, Result, Tool, schemas};
use serde_json::Value;
use sha2::{Digest, Sha256};
use state_store::Arguments;

/// An operation paired with arguments accepted by its fixed tool contract.
/// Fixed fields remain typed; dynamic fields hold agent-defined data.
#[derive(Clone)]
pub struct Request {
    arguments: Arguments,
    receipt_hash: Option<String>,
}

impl Request {
    pub fn parse(name: &str, arguments: Value) -> Result<Self> {
        let (tool, arguments) = schemas::normalize_call(name, arguments)?;
        Self::grouped(tool, arguments)
    }
    pub fn grouped(tool: Tool, arguments: Value) -> Result<Self> {
        schemas::validate_operation(tool.as_str(), &arguments)?;
        let receipt_hash = arguments.get("idempotency_key").map(|_| {
            format!("{:x}", Sha256::digest(serde_json::json!({"tool":tool.as_str(), "arguments":crate::canonical(&arguments)}).to_string()))
        });
        Ok(Self {
            arguments: Arguments::parse(tool, arguments)?,
            receipt_hash,
        })
    }
    pub fn operation(&self) -> Operation {
        self.arguments.operation()
    }
    pub fn arguments(&self) -> &Arguments {
        &self.arguments
    }
    pub fn receipt_hash(&self) -> Option<&str> {
        self.receipt_hash.as_deref()
    }
    pub(crate) fn into_arguments(self) -> Arguments {
        self.arguments
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileAction;
    use serde_json::json;

    #[test]
    fn public_and_host_requests_preserve_one_contract() {
        let public = Request::parse("fs.read", json!({"namespace":"ns", "path":"/file"})).unwrap();
        let host = Request::grouped(
            Tool::File,
            json!({"action":"read", "namespace":"ns", "path":"/file"}),
        )
        .unwrap();
        assert_eq!(public.operation(), Operation::File(FileAction::Read));
        assert_eq!(public.arguments(), host.arguments());
        assert_eq!(public.operation(), host.operation());
    }

    #[test]
    fn receipt_hashes_preserve_the_wire_request_before_typed_defaults() {
        let omitted = Request::parse(
            "call",
            json!({"namespace":"ns", "function":"f", "idempotency_key":"key"}),
        )
        .unwrap();
        assert_eq!(
            omitted.receipt_hash(),
            Some("53dd4b4da4aa483c1439fccb25803d8dfe52993a5f165ec065c0e982ae626595")
        );
        let explicit = Request::parse(
            "state_call",
            json!({"idempotency_key":"key", "function":"f", "namespace":"ns", "arguments":{}}),
        )
        .unwrap();
        assert_eq!(omitted.arguments(), explicit.arguments());
        assert_ne!(omitted.receipt_hash(), explicit.receipt_hash());
        let reordered = Request::parse(
            "state_call",
            json!({"idempotency_key":"key", "function":"f", "namespace":"ns"}),
        )
        .unwrap();
        assert_eq!(omitted.receipt_hash(), reordered.receipt_hash());
    }

    #[test]
    fn malformed_requests_never_produce_a_parsed_value() {
        for (name, args) in [
            ("fs.read", json!({"namespace":"ns"})),
            ("fs.read", json!({"namespace":"ns", "path":42})),
            (
                "fs.read",
                json!({"namespace":"ns", "path":"/file", "action":"read"}),
            ),
            (
                "fs.write",
                json!({"namespace":"ns", "path":"/file", "text":"x", "base64":"eA=="}),
            ),
            ("describe", json!({"function":"f"})),
            ("state_execute", json!({"source":"1", "unexpected":true})),
        ] {
            assert!(Request::parse(name, args).is_err(), "{name}");
        }
    }
}
