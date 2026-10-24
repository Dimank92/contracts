#![no_std]

use soroban_sdk::{contracterror, contracttype, symbol_short, Address, Env};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum AdminHandoverError {
    Unauthorized = 1,
    NoActiveProposal = 2,
    ProposalExpired = 3,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct PendingAdminProposal {
    pub proposed_admin: Address,
    pub proposed_timestamp: u64,
    pub expiration_timestamp: u64,
}

pub struct AdminHandover;

impl AdminHandover {
    pub const HANDOVER_TIMEOUT_SECS: u64 = 86400 * 2; // 48-hour expiration

    pub fn propose_admin(env: &Env, current_admin: &Address, candidate: &Address) -> Result<(), AdminHandoverError> {
        current_admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&symbol_short!("ADMIN")).unwrap_or_else(|| panic_with_error!(env, Error::InternalInvariant));
        if current_admin != &stored_admin {
            return Err(AdminHandoverError::Unauthorized);
        }

        let now = env.ledger().timestamp();
        let proposal = PendingAdminProposal {
            proposed_admin: candidate.clone(),
            proposed_timestamp: now,
            expiration_timestamp: now + Self::HANDOVER_TIMEOUT_SECS,
        };

        env.storage().instance().set(&symbol_short!("P_ADMIN"), &proposal);
        env.events().publish((symbol_short!("PROP_ADM"), candidate), (proposal.expiration_timestamp,));
        Ok(())
    }

    pub fn accept_admin(env: &Env, candidate: &Address) -> Result<(), AdminHandoverError> {
        candidate.require_auth();
        let proposal: PendingAdminProposal = env.storage().instance().get(&symbol_short!("P_ADMIN"))
            .ok_or(AdminHandoverError::NoActiveProposal)?;

        let now = env.ledger().timestamp();
        if now > proposal.expiration_timestamp {
            env.storage().instance().remove(&symbol_short!("P_ADMIN"));
            return Err(AdminHandoverError::ProposalExpired);
        }

        if candidate != &proposal.proposed_admin {
            return Err(AdminHandoverError::Unauthorized);
        }

        env.storage().instance().set(&symbol_short!("ADMIN"), candidate);
        env.storage().instance().remove(&symbol_short!("P_ADMIN"));
        env.events().publish((symbol_short!("NEW_ADM"), candidate), ());
        Ok(())
    }
}
