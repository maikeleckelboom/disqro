# Disqro engineering

- Use American English in first-party code, interface copy, documentation, and diagnostics.
- Keep one current data model, persistence contract, and implementation per concern. Remove superseded paths, adapters, aliases, historical workspace readers, migrations, and feature flags when their replacements land. Update their callers and tests together.
- Preserve stable logical recording identities, independent browser state, and explicit ownership of asynchronous work. Share interface primitives across surfaces with the same interaction.
- Preserve Favorites, ratings, playlists, listening history, registered collections, and workspace settings. Create disposable profiles directly with the current schema. Refuse incompatible library databases without altering them. Handle actual personal data through an isolated, one-time conversion or export and import with the owner's explicit approval.
- Qualify Windows changes with isolated profiles, silent audio, and the private native desktop tools. Keep the Waveform Native dependency and Windows presentation patch pinned together.
