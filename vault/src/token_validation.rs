use soroban_sdk::{token, Address, Env};

#[derive(Clone, Debug, PartialEq)]
pub enum TokenError {
    InvalidToken,
    ZeroTransfer,
    DeltaMismatch,
}

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
        let received = after_bal
            .checked_sub(before_bal)
            .ok_or(TokenError::DeltaMismatch)?;

        if received < amount {
            return Err(TokenError::DeltaMismatch);
        }

        Ok(received)
    }
}
