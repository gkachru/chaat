# Plugin protocol fixtures

One JSON message per file, exactly as sent on the wire (one line). Kept
identical in the Chaat repo (`tests/fixtures/plugin-protocol/`) and the
call-alerts repo (`tests/fixtures/plugin-protocol/`). Each side's tests
parse the messages it receives and check that the messages it sends equal
these (as JSON values; key order doesn't matter). Change both copies together.

Chaat to plugin: hello, accounts, forget, sign_in, options, presence, action, shutdown.
Plugin to Chaat: ready, ready_options (a ready with needs and options), ready_options_rich (a ready with every option kind, format, limit and `visible_when`), sign_in_required, signed_in, listening, incoming_call, error, option_status, options_update.
