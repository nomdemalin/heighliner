// Tiering helpers. The current retier policy lives in TierPolicy::tier_for_age
// and the sweep logic is in scheduler::retier_sweep; this module is left as a
// hook for future retier strategies (per-rule overrides, user pins, etc).
