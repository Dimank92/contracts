# Multisig signer weights

Every `multisig` signer carries a `u32` weight. An action is approved once the
summed weight of its fresh approvals reaches `threshold`.

- **Backwards compatible:** `initialize` gives every signer weight 1, so
  `threshold` is a plain M-of-N count. `is_approved`, `is_executable(action_id, class)`
  and the approval storage keys behave as before.
- **Weighted:** `initialize_weighted` takes `(address, weight)` pairs. Zero weights
  (`InvalidWeight`), a total that overflows `u32` (`WeightOverflow`) and a
  threshold above the total weight (`InvalidThreshold`) are rejected with distinct errors.
- **Changing weights:** a weight change is a signer-set rotation (`SignerChange`
  adding an existing address with a new weight). It is approved at the rotation
  threshold, delayed, and bumps the signer epoch, which invalidates every
  outstanding approval.
- **Single-key quorum policy:** a signer whose weight alone meets `threshold` is
  permitted, and announced with a `single_key_quorum` event.
- Approval TTL, per-class delays and `__check_auth` encoding are unchanged.
