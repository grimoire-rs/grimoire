# Conformance tree

The index tree the grimoire indexer emits for a fixed seed, checked in so
grim's tests read exactly what the indexer publishes. `MANIFEST.json` lists
every file but itself and this README with its SHA-256 and size (of the inflated bytes for `.gz`),
and the indexer commit the tree came from.

Never hand-edit anything here: a changed byte fails the manifest check in
`src/catalog/conformance_tests.rs`. To regenerate, in the indexer checkout at
the SHA recorded in `MANIFEST.json`:

```sh
npm run build && node scripts/gen-conformance.mjs --out <empty dir>
node scripts/gen-conformance.mjs --check <dir>/MANIFEST.json
```

Then replace every other file in this directory with the output, so this README
survives.
