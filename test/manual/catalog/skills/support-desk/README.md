# support-desk

Repository README for the manual rig's annotation showcase. It is published
as the repository's **description companion** (the reserved `__grimoire`
tag), together with the `[description.support]` channels declared in
`catalog/publish.toml`.

Read it back with:

```sh
grim fetch localhost:5050/grimoire/skills/support-desk --description
grim describe localhost:5050/grimoire/skills/support-desk --format json
```

Editing the support links in `publish.toml` and re-running `bootstrap.sh`
re-points this companion and changes the answer for **every** published
version — the artifact manifests are untouched.
