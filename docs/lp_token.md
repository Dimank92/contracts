# LP Token: SEP-41 conformance

`lp_token` is the share token for `lp_pool`. `initialize(pool, decimal,
name, symbol)` pins the pool. It requires the pool's own authorization,
so nobody can claim a freshly deployed token for a pool that didn't
agree to it. Only the pinned pool can `mint(to, amount)` and
`burn(from, amount)`.

`initialize` is idempotent-safe: the pool pin lives in instance storage
and a second call fails with `Error::AlreadyInitialized` rather than
silently re-pinning the pool or resetting the metadata.

## Pool rotation

The pool pin can only change through `set_pool(new_pool)`, an explicit
admin-gated rotation path. It requires the current admin's authorization
and emits a `set_pool` event with topics `[set_pool]` and data
`(old_pool, new_pool)`. There is no other way to change the mint/burn
authority, so the share supply always has exactly one minter.

## Why a direct implementation, not OpenZeppelin `fungible`

The token implements `soroban_sdk::token::Interface` in a single file,
following the reference `soroban-examples/token` layout. Every Zenith
contract pins soroban-sdk 21, the interface is small, and doing it
directly keeps the audited surface to this crate with no extra
dependency. Migrating to OpenZeppelin's `fungible` module stays an
option once the workspace moves to a matching SDK.

## Conformance checklist

| SEP-41 requirement | Status | Test |
|---|---|---|
| `allowance(from, spender)` returns 0 once the expiration ledger has passed | ✅ | `allowance_expires_at_its_expiration_ledger` |
| `approve(from, spender, amount, expiration_ledger)` requires `from` auth, emits `approve` with topics `[approve, from, spender]` and data `(amount, expiration_ledger)` | ✅ | `transfer_approve_transfer_from_burn` |
| `approve` rejects a nonzero amount with a past `expiration_ledger`, but accepts it for a zero amount | ✅ | `a_nonzero_allowance_cannot_be_already_expired`, `a_zero_allowance_may_carry_a_past_expiration` |
| `balance(id)` | ✅ | all |
| `transfer(from, to, amount)` requires `from` auth, emits `[transfer, from, to]` / `amount` | ✅ | `transfer_approve_transfer_from_burn` |
| `transfer_from(spender, from, to, amount)` requires `spender` auth and consumes allowance | ✅ | `transfer_approve_transfer_from_burn`, `transfer_from_over_allowance_fails`, `transfer_from_an_expired_allowance_fails` |
| `burn(from, amount)` requires `from` auth, emits `[burn, from]` / `amount` | ✅ | `transfer_approve_transfer_from_burn` |
| `burn_from(spender, from, amount)` requires `spender` auth and consumes allowance | ✅ | `transfer_approve_transfer_from_burn`, `burn_from_over_allowance_fails` |
| `decimals`, `name`, `symbol` | ✅ | `metadata` |
| Negative amounts rejected | ✅ | `negative_amounts_are_rejected` |
| Works through the generic `token::TokenClient` | ✅ | `works_through_the_generic_token_client` |
| `mint` restricted to the pinned pool; emits `[mint, pool, to]` / `amount` | ✅ | `mint_requires_the_pool_and_emits_mint`, `mint_without_pool_auth_fails` |
| `burn` restricted to the pinned pool; burning more than the balance fails | ✅ | `burn_requires_the_pool`, `burn_over_balance_fails` |
| `initialize` cannot be called twice; the pool pin is never reset | ✅ | `initialize_is_idempotent_safe`, `reinitialization_cannot_repin_the_pool` |
| `set_pool` rotates the pin only with admin auth and emits `set_pool` | ✅ | `set_pool_requires_admin`, `set_pool_rotates_mint_authority` |
| Self-transfer and transfer to the contract's own address behave as plain transfers | ✅ | `self_transfer_and_transfer_to_contract` |
| `transfer_from` with an allowance exactly equal to the amount succeeds and zeroes the allowance | ✅ | `transfer_from_exact_allowance` |
| Approving the zero address is allowed and readable via `allowance` | ✅ | `approve_zero_address` |

## TTL management

- **Instance** (pool, metadata): extended on every state-changing call.
- **Balances**: persistent. Extended to 90 days whenever read or
  written, once fewer than 60 days remain.
- **Allowances**: temporary storage, live exactly until
  `expiration_ledger`. An expired allowance reads as zero, and the
  network drops the entry without anyone paying to delete it.
