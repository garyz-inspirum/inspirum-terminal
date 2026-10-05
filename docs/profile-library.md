# Saved SSH profile library

## Editing and transfer

Select a saved profile to edit it. Changing its name and choosing **Save profile** renames the selected entry; a collision with another name is rejected. **Duplicate** creates an editable draft with a unique name and does not save until requested. **Delete** requires confirmation and never disconnects an already-open SSH/SFTP tab.

**Export all** writes validated JSON without overwriting an existing file. **Import + merge** rejects colliding names. **Replace from file** requires explicit confirmation of the selected path and replaces the library only after validation and atomic save succeed. An explicit JSON array `[]` is a valid empty replacement; a missing file, zero-byte file, directory, malformed JSON, unsafe field or oversized result is an error, never an empty replacement.

Imports open their source once. The startup convenience where a missing active profile store means a new empty library does not apply to an import. Source input and the complete resulting library are limited to 1 MiB and 1,000 profiles, including when two individually valid libraries are merged. Existing files and in-memory state stay unchanged on failure.

## Security and portability

Exports omit credential/key contents but contain operational metadata, including hostnames, usernames, local identity paths, routes and optional remote commands. Remote commands can themselves contain sensitive text supplied by the user: review them before sharing an export. Identity files, agents and trusted host databases are not copied. Review imported remote commands and routes before connecting. Import never opens a connection or executes a command.

## Verification

`cargo test --locked --test profile_import_safety` covers missing/empty/malformed/oversized sources, directory rejection, combined-size limits, invalid existing profiles and preservation of the active library. Existing profile tests continue to cover name collisions, no-clobber exports and round trips. Native CI is required on Linux x64, Windows x64 and macOS arm64 before merging this increment (issue #29). No change to the broader SSH acceptance claims.
