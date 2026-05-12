use thiserror::Error;

#[derive(Debug, Error)]
pub enum LandedTxError {
    #[error("RPC error: {0}")]
    Rpc(#[from] solana_client::client_error::ClientError),

    #[error("no recent prioritization fee samples returned by RPC")]
    NoFeeSamples,

    #[error("transaction did not land after {attempts} attempts")]
    NotLanded { attempts: u32 },

    #[error("signer error: {0}")]
    Signer(String),
}

pub type Result<T> = std::result::Result<T, LandedTxError>;
