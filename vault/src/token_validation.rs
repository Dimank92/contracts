use soroban_sdk::{token, Address, Env};

#[derive(Clone, Debug, PartialEq)]
/// Why a guarded token transfer was rejected.
pub enum TokenError {
    /// The token contract misbehaved or is not a valid token.
    InvalidToken,
    /// The requested amount was zero or negative.
    ZeroTransfer,
    /// The observed balance change did not match the requested amount.
    DeltaMismatch,
}

/// Guards inbound collateral transfers against fee-on-transfer and misbehaving tokens.
pub struct TokenTransferGuard;

impl TokenTransferGuard {
    /// Validates collateral token transfers and verifies that the contract's actual
    /// received balance delta matches the requested amount (Checks-Effects-Interactions).
    pub fn safe_inbound_transfer(
        env: &Env,
        token: &Address,
        from: &Address,
        recipient: &Address,
        amount: i128,
    ) -> Result<i128, TokenError> {
        if amount <= 0 {
            return Err(TokenError::ZeroTransfer);
        }

        let client = token::Client::new(env, token);
        let before_bal = client.balance(recipient);

        client.transfer(from, recipient, &amount);

        let after_bal = client.balance(recipient);
        let received = after_bal.checked_sub(before_bal).ok_or(TokenError::DeltaMismatch)?;

        if received < amount {
            return Err(TokenError::DeltaMismatch);
        }

        Ok(received)
    }
}
