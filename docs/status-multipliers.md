# Multipliers branch: status and handoff (2026-10-29)

Branch `multipliers`, worked on together with chipsmith's `multipliers`
(`/pool/dev/chipsmith-wt/mult`). chipsmith's `docs/status-multipliers.md`
has the full status; this note covers what's specific to this repo.

## Done and verified

- master (97fd97e) is merged in (e490412).
- The chip database has the MULT tile with registers: clock and clear
  fast muxes, row clocks `BCB0-5`, the enable. `cram chipdb` output is
  byte-equal to the committed `db/EP2C35F672C6/chipdb.txt.xz`
  (2026-10-29).
- `cram quartus-export`:
  - writes the `mac_mult` + `mac_out` atoms with registers, inverted data
    pins and `~GND`.
  - (f0406b4) An inverting feeder from a register or a pin becomes
    Quartus' `<src>~_wirecell`, placed in the feeder's LE.

  With these, chipsmith's `fir` and `mult-selftest` are byte-identical in
  Quartus 13.0sp1. Before f0406b4, fir lost 100 rcf routes to wirecells
  Quartus re-placed.

## Not verified / not started

- The wirecell change touches every design with such a feeder. The M4K
  and earlier byte-identical examples were not re-validated with it, and
  neither were uart and i2c-master, which it may fix.
- The README "Embedded multipliers" section (in the WIP commit 46e5447)
  says chipsmith's README has the list of byte-identical designs; that
  list isn't written yet.
- `tools/cram/Cargo.toml` and `tools/ddb/Cargo.toml` point at the
  chipsmith worktree by path. Before merge:
  1. Switch both to the git rev of chipsmith's `multipliers` head.
  2. Run `cargo update -p chipsmith-cyclone2` in both.
  3. Update `nix/cram.nix`'s outputHash.
- `tools/ddb/Cargo.lock` still names rev 816ddd5 from the merge.

## Commands

```
cd /pool/dev/cyclone2-re-wt/mult
systemd-run --user --scope -q -p MemoryMax=20G -p MemorySwapMax=0 nix develop --command cargo build --release --manifest-path tools/cram/Cargo.toml
tools/cram/target/release/cram chipdb /tmp/c.txt && xz -dc db/EP2C35F672C6/chipdb.txt.xz | cmp - /tmp/c.txt
# Quartus check of a chipsmith build: chipsmith's bench/quartus-validate.sh with CYCLONE2_RE=$PWD
```
