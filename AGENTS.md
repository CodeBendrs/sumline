# Project instructions

Use the code-review-graph MCP tools before exploring this codebase with file searches or reads. Start with `semantic_search_nodes`/`query_graph` for code, `get_architecture_overview` for architecture, and `detect_changes` plus `get_affected_flows` for reviews. Use `query_graph` with `tests_for` to inspect coverage. Fall back to direct file inspection when the graph is unavailable or does not cover the needed code; Rust test attributes may not be recognized by the graph.

Keep authorization at workspace boundaries. Route individual invoices, never an entire email, to stores. Uncertain store matches, dates, currencies, totals, and bundle boundaries must require review. Amounts use integer cents. Do not execute invoice links or content, log complete documents/tokens, or commit real PDFs, databases, keys, exports, or `.env` files.

Run `cargo fmt --check`, `cargo test --locked`, and `cargo clippy --locked --all-targets -- -D warnings` after material changes. `tests/smoke.py` checks a running local preview; it creates and deletes only its own test fixtures. Do not claim live OAuth, deployment, legal compliance, or production security without corresponding validation.
