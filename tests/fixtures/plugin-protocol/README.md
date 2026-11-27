# Plugin protocol fixtures

One JSON message per file, exactly as sent on the wire (one line). Chaat's
tests parse the messages it receives and check that the messages it sends
equal these (as JSON values; key order doesn't matter). A plugin can copy
these files into its own tests to check the same messages from its side.

Chaat to plugin: hello, accounts, forget, sign_in, options, presence, action, push_channels, shutdown.
Plugin to Chaat: ready, ready_options (a ready with needs and options), ready_options_rich (a ready with every option kind, format, limit and `visible_when`), sign_in_required, signed_in, listening, incoming_call, error, option_status, options_update, push_ready (the receiver's public https address), push (one forwarded notification body on a channel), push_down (the receiver stopped), agent_query (a read request), agent_propose (a reply proposal).
