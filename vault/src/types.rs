use soroban_sdk::{contracttype, Address};

/// Storage keys for the vault.
#[contracttype]
#[derive(Clone, PartialEq)]
pub enum DataKey {
    Admin,
    Token,    // default token
    Paused,
    /// Escrow balance per (token, series_id) — the per-tag ledger.
    Escrow(Address, u64),
    /// Total escrowed per token.
    TotalEscrowed(Address),
}

