# Setup file

A setup file (`.chaatsetup`) carries the settings a person needs on first run, so they do not type IDs or an access key. An administrator creates it once in Settings and passes it on. It is encrypted with a passphrase.

## What is in it

- Organization name, shown as "Set up for {org}".
- Application (client) ID and directory (tenant) ID of the app registration.
- Optionally, "only allow accounts from this tenant". After sign-in, an account from another tenant gets the toast "This account is from a different organization than {org}'s setup file."
- Optionally, the push relay address and its access key ([realtime.md](realtime.md)).
- The creation date and an optional "Stop accepting after" date. Chaat refuses a file past that date, so you can retire files after a key rotation.

It holds no tokens, passwords or message data.

## Create a file

1. Open Settings > Advanced > Setup file and select **Create a setup file…**.
2. Enter the organization name (required). Choose whether to limit accounts to the tenant, whether to include the push relay (offered only when a relay address and access key are set), and an optional "Stop accepting after" date (YYYY-MM-DD, today or later).
3. Select **Create file…** and pick where to save. The default name is `Chaat setup.chaatsetup`.
4. Chaat shows a generated passphrase of six words once. Select **Copy**, then **Done**; the passphrase is cleared.

Send the passphrase separately from the file, for example by phone or another chat.

## Import a file

- On first run: drop the file on the window, or select **Choose file**. Enter the six-word passphrase (**Show** reveals it to check for typing mistakes; after a wrong passphrase the words stay in the field so you can correct them) and select **Open**. Check the summary (who can sign in, whether real-time updates are included; **Details** shows the IDs), then select **Continue to sign in**. Microsoft's sign-in page opens in your browser. After the first sign-in Chaat shows "Chaat is set up for {org}." once. **Enter details manually** replaces the drop zone with the registration form. On the sign-in step, **Use a different setup file** goes back to step one.
- Later: Settings > Advanced > Setup file > **Import a setup file…**. If accounts are signed in and the client ID or tenant would change, the review warns "Applying signs every account in again."

The summary shows the organization, who can sign in, whether real-time updates are included and any "Accepted until" date; **Details** holds the client ID, tenant and creation date. Applying overrides only the settings the file carries (client ID, tenant, organization, expected tenant, and the relay address and access key when included) and keeps everything else. The access key goes into the OS credential store. Accounts sign in again only when the client ID or tenant changed.

Error messages: "This isn't a Chaat setup file." "This file needs a newer version of Chaat." "This file can't be opened with that passphrase, or it was changed." "This file can't be read. Ask for a new one." "This setup file expired on {date}. Ask for a new one."

## Try the onboarding on a configured Chaat

Start Chaat with `CHAAT_ONBOARDING=1` to see the welcome screen at launch even when it is configured and signed in. On Windows (PowerShell): `$env:CHAAT_ONBOARDING = "1"; chaat`. On macOS and Linux: `CHAAT_ONBOARDING=1 chaat`.

- The screen is exactly what a new person sees; nothing on it mentions the variable.
- A file applied there overrides only the settings it carries, as above. Your accounts, plugins and other settings stay.
- Applying a file or saving the manual form ends it. To leave without either, quit Chaat and start it without the variable.
- It applies to that launch only. Chaat runs one copy at a time, so quit a running Chaat (including from the tray) first.

## Protection

The file is encrypted with XChaCha20-Poly1305. The key comes from the passphrase through Argon2id. A wrong passphrase and a changed file give the same error. Passphrases are six random words from the EFF large wordlist. Chaat never uploads the file, and never logs the passphrase, paths or contents. The access key goes only to the service backend, not the interface.

## Limits

- Everyone who imports a file shares one relay access key.
- To revoke one person, rotate the key on the relay and send everyone a new file. Set "Stop accepting after" on the old files if you can.
- Anyone with the file and passphrase can read the key, and anyone who imported it can read it from their own credential store. The passphrase protects the file in transit, not the key on a person's machine.
