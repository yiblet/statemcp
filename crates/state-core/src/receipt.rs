use crate::{Error, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
#[derive(Clone, Debug)]
pub struct ReceiptIdentity {
    key: String,
    hash: String,
}
impl ReceiptIdentity {
    pub fn for_arguments(key: &str, tool: &str, arguments: &impl Serialize) -> Result<Self> {
        if key.is_empty() || key.len() > 256 {
            return Err(Error::invalid("idempotency_key must contain 1..256 bytes"));
        }
        #[derive(Serialize)]
        struct Fingerprint<'a, T: Serialize> {
            arguments: &'a T,
            tool: &'a str,
        }
        let bytes = serde_json::to_vec(&Fingerprint { arguments, tool })
            .map_err(|e| Error::invalid(e.to_string()))?;
        Ok(Self {
            key: key.to_owned(),
            hash: format!("{:x}", Sha256::digest(bytes)),
        })
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn hash(&self) -> &str {
        &self.hash
    }
    pub(crate) fn check(key: Option<&str>, identity: Option<&Self>) -> Result<()> {
        if key != identity.map(Self::key) {
            return Err(Error::invalid(
                "idempotency key requires its matching receipt identity",
            ));
        }
        Ok(())
    }
}
