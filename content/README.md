# `content/`: the data we author

This directory holds the tables **we invent**: policy and tuning that no
client file gives us. The full policy is `docs/content/plan.md` §2; the
header line of every file is frozen in `docs/content/schema.md` §4. Read
both before you add a file.

## What may go here, and what may not

| class | what | where |
|---|---|---|
| A: protocol vocabulary | ids and constants the wire and logic need (group ids, enum values, attribute ids) | Rust source, cited [V] |
| B: client content rows | anything read out of the client: type numbers, materials, blueprints, names, counts | `out/content/` only, written by `tools/content.py`. **Never here, never in the repo** |
| C: what we invent | policy tables and tuning | **here**, as TSV (a table), or a Rust const (a single number) |

- **No extracted client tables.** No extracted row set, count or name list.
  If a table came out of the client, it belongs in the bundle, not in this
  directory. Individual factual client ids are allowed where a row needs one
  (`econ/hubs.tsv`, `facwar/warzones.tsv`, `econ/starter-fit.tsv` and
  `wormholes/statics.tsv` are small examples).
- **A table or a const?** A value keyed by band, level, faction or region is
  a row here. A single number is a Rust const with an `[A]` doc comment.
- **Controller-tuned values are world deltas, not files** (`m_ask`, `m_bid`,
  `DBS_GLOBAL`, the trading index).

## The format

- UTF-8, `\n` line ends, tab-separated. No field holds a TAB.
- Lines starting with `#` are comments and may go anywhere, including above
  the header, so a file can open with its `[A]` preamble.
- **The header is the first line that is neither empty nor a comment**: the
  column names joined by single TABs, with no `#`. It must match
  `schema.md` §4 byte for byte. A file whose header differs is an error at
  startup, never a silent fallback.
- Every following non-comment line is a row. A row with the wrong number of
  fields, or a field its reader cannot parse, is **skipped and logged** with
  its line number. Watch the startup output after an edit.
- **Every `[A]` row carries a `why` column, last, with its falsifier**: what
  observation would prove the value wrong.

## How the server reads a file

The loader is `server/crates/eve-server/src/authored.rs` (plan §1.4).

- Each reader compiles its file in with `include_str!`, so the binary always
  has the shipped defaults.
- At runtime the server reads `--authored DIR` (default `content`, relative
  to the working directory; `tools/up.sh` passes it). A file found there
  **wins** over the compiled-in copy, so you can retune without a rebuild.
  A file it lacks uses the compiled-in copy.
- **The startup line names the copy used for each file**:
  `[authored] <file>: disk <path> (N rows, M skipped)` or
  `[authored] <file>: compiled-in (...)`.

## Adding a file

1. Add its header and column table to `schema.md` §4 first, in the same
   landing as the writer and every reader (a header change is a schema
   change).
2. Keys that become delta keys (for example `population.tsv`'s `key`) are
   **never renamed**: a rename drops that row's live deltas. Retire a row by
   deleting it and add a new key.
3. Tests may parse a shipped file for well-formedness. They may **not**
   assert any value in it; a test that needs rows uses invented ones.
4. A file an extractor reads (an override such as
   `npc/region-pirates.tsv`) is applied at extract time and recorded as an
   `input` line in the bundle's manifest, so editing it re-extracts.
