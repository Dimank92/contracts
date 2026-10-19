#![no_std]

//! Zenith Streams — linear, per-second token streams with an optional
//! cliff, funded upfront by the funder (typically the treasury).
//!
//! `streamed(t) = total × (t − start) / (end − start)`, floored, clamped to
//! `[0, total]` and zero before `cliff`. Because every intermediate value
//! is floored and the recipient can only ever withdraw
//! `streamed(now) − withdrawn`, the total paid out can never exceed the
//! deposit (see the property tests in test.rs).
//!
//! The recipient withdraws at any time and may transfer the stream to a
//! new recipient. The funder or governance can cancel: the recipient is
//! paid everything accrued but not yet withdrawn, and the funder is
//! refunded the unstreamed remainder.

use soroban_sdk::{contract, contractimpl, token, Address, Env};

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_events;

mod error;
mod events;
pub mod math;
mod migrations;
mod types;

pub use error::Error;
pub use events::{
    InitializedEvent, StreamCanceledEvent, StreamCreatedEvent, StreamTransferredEvent,
    WithdrawnEvent,
};
pub use migrations::CURRENT_SCHEMA_VERSION;
use types::DataKey;
pub use types::Stream;

zenith_common::contract_meta!();

#[contract]
pub struct Streams;

fn load(env: &Env, stream_id: u64) -> Result<Stream, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Stream(stream_id))
        .ok_or(Error::StreamNotFound)
}

fn save(env: &Env, stream_id: u64, stream: &Stream) {
    env.storage()
        .persistent()
        .set(&DataKey::Stream(stream_id), stream);
}

fn governance(env: &Env) -> Result<Address, Error> {
    env.storage()
        .instance()
        .get(&DataKey::Governance)
        .ok_or(Error::NotInitialized)
}

/// Amount streamed to the recipient as of `now`, in `[0, total]`.
fn streamed_at(stream: &Stream, now: u64) -> Result<i128, Error> {
    if stream.canceled || now >= stream.end {
        return Ok(stream.total);
    }
    if now < stream.cliff || now <= stream.start {
        return Ok(0);
    }
    math::mul_div_floor(
        stream.total,
        (now - stream.start) as i128,
        (stream.end - stream.start) as i128,
    )
    .ok_or(Error::MathOverflow)
}

#[contractimpl]
impl Streams {
    pub fn initialize(env: Env, governance: Address) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Governance) {
            return Err(Error::AlreadyInitialized);
        }
        env.storage()
            .instance()
            .set(&DataKey::Governance, &governance);
        env.storage().instance().set(&DataKey::StreamCounter, &0u64);
        zenith_common::migrations::init_schema(&env, CURRENT_SCHEMA_VERSION);
        events::initialized(
            &env,
            InitializedEvent {
                governance,
                schema_version: CURRENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    pub fn get_governance(env: Env) -> Result<Address, Error> {
        governance(&env)
    }

    /// Creates a stream and pulls `total` of `token` from `funder` upfront.
    /// `start` may be in the past (the elapsed portion is withdrawable
    /// immediately, subject to the cliff). Zero-duration streams
    /// (`start == end`) are rejected.
    #[allow(clippy::too_many_arguments)]
    pub fn create_stream(
        env: Env,
        funder: Address,
        recipient: Address,
        token: Address,
        total: i128,
        start: u64,
        cliff: u64,
        end: u64,
    ) -> Result<u64, Error> {
        funder.require_auth();
        migrations::require_current(&env)?;
        if total <= 0 {
            return Err(Error::InvalidAmount);
        }
        if start >= end || cliff < start || cliff > end {
            return Err(Error::InvalidSchedule);
        }

        token::Client::new(&env, &token).transfer(&funder, &env.current_contract_address(), &total);

        let stream_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::StreamCounter)
            .ok_or(Error::NotInitialized)?;
        let next = stream_id.checked_add(1).ok_or(Error::MathOverflow)?;
        env.storage().instance().set(&DataKey::StreamCounter, &next);

        save(
            &env,
            stream_id,
            &Stream {
                funder: funder.clone(),
                recipient: recipient.clone(),
                token: token.clone(),
                total,
                start,
                cliff,
                end,
                withdrawn: 0,
                canceled: false,
            },
        );
        events::created(
            &env,
            stream_id,
            StreamCreatedEvent {
                funder,
                recipient,
                token,
                total,
                start,
                cliff,
                end,
            },
        );
        Ok(stream_id)
    }

    /// Recipient withdraws up to their currently accrued balance.
    pub fn withdraw(env: Env, stream_id: u64, amount: i128) -> Result<(), Error> {
        let mut stream = load(&env, stream_id)?;
        stream.recipient.require_auth();
        migrations::require_current(&env)?;
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let available = streamed_at(&stream, env.ledger().timestamp())? - stream.withdrawn;
        if amount > available {
            return Err(Error::InsufficientBalance);
        }
        // amount <= streamed - withdrawn, so this stays <= total.
        stream.withdrawn += amount;
        save(&env, stream_id, &stream);

        token::Client::new(&env, &stream.token).transfer(
            &env.current_contract_address(),
            &stream.recipient,
            &amount,
        );
        events::withdrawn(
            &env,
            stream_id,
            WithdrawnEvent {
                recipient: stream.recipient,
                amount,
                withdrawn: stream.withdrawn,
            },
        );
        Ok(())
    }

    /// Funder or governance cancels. The recipient receives everything
    /// accrued but not yet withdrawn (zero if still before the cliff) and
    /// the funder is refunded the unstreamed remainder.
    pub fn cancel(env: Env, caller: Address, stream_id: u64) -> Result<(), Error> {
        caller.require_auth();
        migrations::require_current(&env)?;
        let mut stream = load(&env, stream_id)?;
        if caller != stream.funder && caller != governance(&env)? {
            return Err(Error::Unauthorized);
        }
        if stream.canceled {
            return Err(Error::StreamCanceled);
        }

        let streamed = streamed_at(&stream, env.ledger().timestamp())?;
        let to_recipient = streamed - stream.withdrawn;
        let to_funder = stream.total - streamed;

        stream.total = streamed;
        stream.withdrawn = streamed;
        stream.canceled = true;
        save(&env, stream_id, &stream);

        let token = token::Client::new(&env, &stream.token);
        let this = env.current_contract_address();
        if to_recipient > 0 {
            token.transfer(&this, &stream.recipient, &to_recipient);
        }
        if to_funder > 0 {
            token.transfer(&this, &stream.funder, &to_funder);
        }
        events::canceled(
            &env,
            stream_id,
            StreamCanceledEvent {
                caller,
                to_recipient,
                to_funder,
                streamed,
            },
        );
        Ok(())
    }

    /// Recipient hands the stream (including any accrued, unwithdrawn
    /// balance) to a new recipient.
    pub fn transfer(env: Env, stream_id: u64, new_recipient: Address) -> Result<(), Error> {
        let mut stream = load(&env, stream_id)?;
        stream.recipient.require_auth();
        migrations::require_current(&env)?;
        if stream.canceled {
            return Err(Error::StreamCanceled);
        }
        let old = stream.recipient.clone();
        stream.recipient = new_recipient.clone();
        save(&env, stream_id, &stream);
        events::transferred(
            &env,
            stream_id,
            StreamTransferredEvent {
                from: old,
                to: new_recipient,
            },
        );
        Ok(())
    }

    // ── Schema migrations (docs/migrations.md) ──────────────────────────────

    /// Runs the registered steps `from+1 ..= to`. Governance only. Returns
    /// the version reached (less than `to` if a step paginated).
    pub fn migrate(env: Env, from: u32, to: u32) -> Result<u32, Error> {
        governance(&env)?.require_auth();
        zenith_common::migrations::migrate(
            &env,
            from,
            to,
            CURRENT_SCHEMA_VERSION,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    /// Continues a paginated migration from `cursor`. Permissionless, like
    /// `bump`: the target was fixed by `migrate` and the steps are code in
    /// this wasm, so a keeper can only advance it.
    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> Result<u32, Error> {
        zenith_common::migrations::migrate_batch(
            &env,
            cursor,
            limit,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    pub fn schema_version(env: Env) -> Result<u32, Error> {
        Ok(zenith_common::migrations::schema_version(&env))
    }

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Result<Option<(u32, u32)>, Error> {
        Ok(zenith_common::migrations::migration_state(&env))
    }

    /// `(recipient_withdrawable, funder_refundable)` as of now: what the
    /// recipient could withdraw right now, and what the funder would get
    /// back if the stream were cancelled right now.
    pub fn balance_of(env: Env, stream_id: u64) -> Result<(i128, i128), Error> {
        let stream = load(&env, stream_id)?;
        let streamed = streamed_at(&stream, env.ledger().timestamp())?;
        Ok((streamed - stream.withdrawn, stream.total - streamed))
    }

    pub fn get_stream(env: Env, stream_id: u64) -> Result<Option<Stream>, Error> {
        Ok(env.storage().persistent().get(&DataKey::Stream(stream_id)))
    }

    pub fn get_stream_count(env: Env) -> Result<u64, Error> {
        env.storage()
            .instance()
            .get(&DataKey::StreamCounter)
            .ok_or(Error::NotInitialized)
    }
}
