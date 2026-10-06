# Saved SSH profile library

## Organization

Saved profiles can be grouped with a folder path, tagged with multiple user labels, and pinned as favourites. Search is case-insensitive across profile name, folder, tags, host and user. Favourite profiles are shown first, then sorted by folder and name.

Profile identity is the deterministic combination of **folder + name**. The same profile name can therefore be used in different folders without ambiguity. Moving or renaming a saved profile changes only the saved-library entry; already-open SSH/SFTP tabs keep their own session snapshot and are never disconnected by library edits.

Existing profile JSON remains compatible. Older entries that do not contain `folder`, `tags` or `favorite` migrate in memory to the root folder, no tags and not-favourite. The new metadata is non-secret and is included in normal import/export JSON.

## Editing and transfer

Select a saved profile to edit it. Changing its name or folder and choosing **Save profile** updates the selected entry; a collision with the same name in the same folder is rejected. **Duplicate** creates an editable draft with a unique name in the same folder and does not save until requested. **Delete** requires confirmation and never disconnects an already-open SSH/SFTP tab.

**Export all** writes validated JSON without overwriting an existing file. **Import + merge** rejects only folder/name collisions, so the same name in different folders is supported. **Replace from file** requires explicit confirmation of the selected path and replaces the library only after validation and atomic save succeed. An explicit JSON array `[]` is a valid empty replacement; a missing file, zero-byte file, directory, malformed JSON, unsafe field or oversized result is an error, never an empty replacement.

Imports open their source once. The startup convenience where a missing active profile store means a new empty library does not apply to an import. Source input and the complete resulting library are limited to 1 MiB and 1,000 profiles, including when two individually valid libraries are merged. Existing files and in-memory state stay unchanged on failure.

## Startup behavior

Startup policy is stored separately from exported/imported profile JSON so importing organizational metadata cannot silently change what the application opens on the next launch.

The application supports three startup choices:

- **Start with no session**.
- **Open selected profile on startup**. The selected saved profile is resolved by folder/name identity and uses the normal OpenSSH host-key and authentication policy. No password, passphrase or authentication response is persisted.
- **Load workspace metadata on startup**. This loads the saved workspace layout only. It does **not** reconnect. Reconnection still requires the existing explicit **Restore & reconnect** action and the layout's persisted reconnect permission.

A missing startup settings file defaults to no startup action. Invalid startup settings are ignored with a visible error rather than falling back to a connection.

## Security and portability

Exports omit credential/key contents but contain operational metadata, including hostnames, usernames, local identity paths, routes, organizational metadata and optional remote commands. Remote commands can themselves contain sensitive text supplied by the user: review them before sharing an export. Identity files, agents and trusted host databases are not copied. Review imported remote commands and routes before connecting. Import never opens a connection or executes a command.

Folder names, tags and favourite state are non-secret organizational metadata. Passwords, passphrases, private-key contents and authentication responses are never part of the profile model or startup settings.

## Verification

Portable unit tests cover legacy JSON migration, grouping/tag filtering, duplicate names in different folders, same-folder collision handling, import/export round trips and startup-settings persistence. Native CI is required on Linux x64, Windows x64 and macOS arm64 before merging issue #55.
