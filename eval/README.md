# Penny eval harness

Minimal automated evaluator for the Penny assistant plane. Runs scenarios
from `scenarios.yaml` against a live `gadgetron serve` and writes a markdown
report under `reports/`.

## Requirements

- `gadgetron serve --no-db --config gadgetron.toml` running locally.
- A local API key from `gadgetron key create` exported as `GADGETRON_API_KEY`.
- Python 3.10+ with `requests` and `pyyaml`.

## Usage

```sh
export GADGETRON_API_KEY=gad_live_...
python3 eval/run_eval.py                      # all scenarios
python3 eval/run_eval.py --scenario wiki-roundtrip
python3 eval/run_eval.py --server http://...  # remote instance
```

Exit code: `0` if no regressions, `1` if any `fail` or `unexpected_pass`.

## Outcome codes

- `pass` — all assertions hold, scenario was expected to pass.
- `fail` — assertions failed, scenario was expected to pass. Triages a bug.
- `expected_fail` — assertions failed, scenario has `expected_status: failing`. Silenced.
- `unexpected_pass` — assertions hold, scenario was tagged failing. Flip to passing.

## Adding scenarios

Edit `scenarios.yaml`. Each entry supports:

- `id`, `description`, `prompt` (single user message)
- `timeout_s` (stream wall clock bound)
- `expected_status: failing` to silence a known gap
- `expect`:
  - `finish_reason`
  - `tool_calls_contain` — every listed tool must appear in the tool_use stream
  - `text_contains_any` — any one substring in the assistant text
  - `page_path` — path under `wiki_path` that Penny should create
  - `page_contains` / `page_contains_any` — substrings in the created page
