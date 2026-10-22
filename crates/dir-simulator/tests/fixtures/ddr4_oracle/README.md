# DDR4 independent trace oracle

`check.py` independently checks the selected DDR4 profile against a serialized
controller trace. It uses literal timing values from the pinned ISSI 4Gb DDR4
Rev F source and does not import the Rust timing implementation. The exact
profile, command ledger, source identity, checked trace, and mutation-review
evidence are recorded outside the repository in
`/tmp/dir-remaining-implementation-20261021-evidence/ddr4-oracle/`; the source
PDF itself is not copied into this repository.

Run the integration gate from the repository root with:

```sh
CARGO_TARGET_DIR=/tmp/dir-remaining-implementation-target \
  /home/hideki/.cargo/bin/cargo test -p dir-simulator --test ddr4_oracle
```

The oracle test runs `check.py` on a trace emitted by the real Rust controller.
The trace contains initialization, ACT/PRE, read/write data beats, request
completion, and refresh events, with a simulation end time of 750,000,000 ps.
The requests finish before the first refresh. The monitor checks end-of-trace
refresh debt and gap, then also checks the selected controller's idle refresh
deadlines. That last condition is a stronger fixture/controller policy than
the device's bounded-postponement allowance; it is not a claim that JEDEC
requires zero refresh debt.

The separate 77-row `coverage.json` maps each ledger rule to this independent
monitor, a named Rust assertion, a selected-configuration invariant, or an
explicit coverage gap. The refreshed map classifies 33 rows as direct-oracle,
40 as other Rust boundary, 0 as partial, 4 as selected-configuration
invariants, and 0 as untested. Zero untested rows does not imply complete DDR4
coverage: overlapping guards, configuration assumptions, and the independent
monitor scope remain explicit. A check that passes on the generated trace is
evidence for that trace path; it does not by itself prove every legal command
combination. The seven oracle mutations each assert the precise expected
monitor rejection reason, but they do not exhaustively mutate all 77 rules.

The controller integration suite currently has these 27 named tests:

- `full_initialization_has_real_commands_and_no_cycle_busy_loop`
- `independent_bank_and_bank_group_minima`
- `cas_direction_and_group_turnarounds`
- `precharge_read_write_and_automatic_precharge`
- `physical_read_write_windows_dm_and_checkpoint_continuation`
- `refresh_priority_blocks_transport_until_real_rfc`
- `strict_configuration_queue_bounds_and_refused_commands`
- `intervening_bank_groups_do_not_erase_write_to_read_guards`
- `reset_and_mode_initialization_boundaries_are_atomic`
- `refresh_zq_and_power_state_boundaries`
- `self_refresh_has_distinct_exit_guards`
- `vref_entry_bits_are_ignored_and_later_steps_settle`
- `write_leveling_qoff_feedback_and_exit_mrd_are_distinct`
- `checkpoint_rejects_removed_dq_history_and_changed_mode`
- `startup_checkpoint_deadlines_are_bijective`
- `delayed_level_feedback_cannot_survive_qoff_or_exit`
- `maintenance_self_refresh_and_destructive_reset_use_same_live_target`
- `long_idle_and_queued_bank_conflicts_refresh_without_starvation`
- `explicit_precharge_refresh_and_mrs_each_have_power_entry_boundary`
- `reset_cke_setup_and_selected_continuous_clock_contract`
- `manual_dll_reset_zq_idle_and_power_down_burst_guards`
- `auto_precharge_with_tras_dominant_read_and_other_bank_progress`
- `srx_reentry_requires_real_ref_and_full_rfc_for_pd_and_sr`
- `selected_mode_reserved_bits_and_training_encoding_matrix`
- `later_vref_single_and_multi_code_steps_have_independent_boundaries`
- `leveling_command_legality_pulse_and_setup_hold_aperture`
- `queued_independent_banks_overlap_but_share_ca_and_dq_buses`

The two isolated runtime DDR4 unit guards are
`independently_isolated_maximum_guards_use_late_inverse_boundaries` and
`clock_hold_setup_are_fixed_continuous_clock_invariants`. The oracle integration tests are `independent_all_history_monitor_accepts_actual_ddr_controller_trace`
and `independent_monitor_rejects_early_commands_and_missing_changed_physical_bytes`.

The refresh adds exact Rust boundaries for RESET/CKE setup, PRE/REF/MRS power
entry, selected MR encoding, Vref step waits, write-leveling pulse/aperture,
and SRE reentry. The clock-before-CKE, SRE clock-hold, SRX clock-setup, and
selected clock-latency rows are configuration/API invariants: the profile
rejects unsupported clock settings and command kinds, while external clock
stops are not executable in this model. RDA/WRA-to-ACT are tested at the
composed internal-precharge plus tRP boundary (one CK early and exact).
`PD_duration_min` has the same effective six-CK interval as `CKE_pulse`;
the five-CK rejection correctly reports the first guard, and six CK succeeds.
These are actual timing behavior boundaries, while unique guard-name isolation
and independent Python coverage remain absent. Guard ordering is unchanged.

The monitor's scope is deliberately limited to the selected initialization,
open-page command, data-window, byte-mapping, completion, and refresh trace.
Power-down, self-refresh, Vref training, write leveling, raw auto-precharge,
analog/electrical behavior, and broader JEDEC conformance require separate
evidence. Selected-profile clock and timing assumptions are listed in the
ledger; this oracle is not a full DDR4 compliance checker.
