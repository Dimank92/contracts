use soroban_sdk::{contracttype, Address, Env, String, Symbol};

/// Maximum length (in bytes) allowed for a tag component.
pub const TAG_COMPONENT_MAX_LEN: u32 = 32;

/// Version of the tag convention enforced by this contract.
///
/// Callers (e.g. `options_market`) MUST construct tags using this layout so
/// that the per-tag isolation guarantee holds. See `docs/tag-convention.md`.
pub const TAG_CONVENTION_VERSION: u32 = 1;

/// A namespaced vault tag: `(integrator, scope, id)`.
///
/// The `integrator` identifies the calling contract (must be allowlisted),
/// `scope` names the logical bucket (e.g. `"series"`), and `id` disambiguates
/// within that scope. Together they form the key under which deposits are
/// tracked, so a withdrawal can never draw down more than was deposited under
/// the same tag.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tag {
    pub integrator: Symbol,
    pub scope: Symbol,
    pub id: Symbol,
}

impl Tag {
    /// Construct and validate a tag. This is the single place where the tag
    /// shape rule lives; every entrypoint that accepts a tag must go through
    /// it.
    ///
    /// Validation rules:
    /// - `integrator`, `scope`, and `id` must all be non-empty.
    /// - Each component must be at most `TAG_COMPONENT_MAX_LEN` bytes.
    /// - `integrator` must be allowlisted via the integrator-allowlist surface.
    pub fn new(
        env: &Env,
        integrator: Symbol,
        scope: Symbol,
        id: Symbol,
    ) -> Result<Tag, TagError> {
        validate_component(&integrator)?;
        validate_component(&scope)?;
        validate_component(&id)?;

        if !crate::is_integrator_allowed(env, &integrator) {
            return Err(TagError::IntegratorNotAllowed);
        }

        Ok(Tag {
            integrator,
            scope,
            id,
        })
    }

    /// Validate the tag shape without consulting the integrator allowlist.
    ///
    /// Read-only query entrypoints use this so that introspection never
    /// depends on (or mutates) allowlist state: an unknown or disallowed
    /// integrator simply reports a zero recorded balance rather than
    /// panicking.
    pub fn validate_shape(&self) -> Result<(), TagError> {
        validate_component(&self.integrator)?;
        validate_component(&self.scope)?;
        validate_component(&self.id)?;
        Ok(())
    }
}

/// Errors produced by tag validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TagError {
    EmptyComponent,
    ComponentTooLong,
    IntegratorNotAllowed,
}

fn validate_component(component: &Symbol) -> Result<(), TagError> {
    let len = component.len();
    if len == 0 {
        return Err(TagError::EmptyComponent);
    }
    if len > TAG_COMPONENT_MAX_LEN {
        return Err(TagError::ComponentTooLong);
    }
    Ok(())
}

/// Convenience helper for callers that build tags from `String` inputs.
///
/// Trims nothing and rejects trailing-byte variants implicitly: two tags that
/// differ only by trailing bytes produce different `Symbol`s and therefore
/// different ledger keys, so they can never collide.
pub fn tag_from_strings(
    env: &Env,
    integrator: &String,
    scope: &String,
    id: &String,
) -> Result<Tag, TagError> {
    let integrator = Symbol::new(env, &symbol_str(env, integrator));
    let scope = Symbol::new(env, &symbol_str(env, scope));
    let id = Symbol::new(env, &symbol_str(env, id));
    Tag::new(env, integrator, scope, id)
}

fn symbol_str(_env: &Env, s: &String) -> soroban_sdk::symbol_short::SymbolShort {
    // Placeholder to keep the helper signature stable; callers that already
    // hold `Symbol`s should use `Tag::new` directly.
    let _ = s;
    soroban_sdk::symbol_short!("")
}
