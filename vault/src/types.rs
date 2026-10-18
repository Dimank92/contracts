use soroban_sdk::contracttype;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Token,
    Paused,
    /// Per-tag escrow ledger. `tag` is caller-defined — options_market uses
    /// its own series_id, so the vault can tell "funds earmarked for series
    /// 7" apart from "funds earmarked for series 8" instead of pooling
    /// everything into one undifferentiated balance.
    Escrow(u64),
    TotalEscrowed,
}
