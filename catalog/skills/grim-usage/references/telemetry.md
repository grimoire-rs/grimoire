# Opt-In Telemetry

You loaded this file because someone wants to turn grim's usage telemetry on,
prove it is off, or see exactly what would leave the machine. It is off by
default, there is no default endpoint and no maintainer-run collector, and
every record goes only to the collector you configure. The whole surface is
**Unstable until 1.0**: switch names and the event and attribute set may
change in a minor release ([stability][stability]). Needs grim 0.15.0 or
later.

## Turn it on

Telemetry is armed only when `GRIM_TELEMETRY` is truthy (`1`, `true`, `yes`,
`on`, any case) **and** no disarm switch applies. `OTEL_*` variables alone never
arm anything. An OTLP endpoint is also **required**:

```sh
export GRIM_TELEMETRY=1
export OTEL_EXPORTER_OTLP_ENDPOINT=https://collector.example.com:4318   # grim appends /v1/logs
grim install
```

- `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` is a full URL used as written and wins
  over the base endpoint. With neither set, an armed run sends nothing.
- The URL must be `http` or `https` with no userinfo; credentials go in
  `OTEL_EXPORTER_OTLP_HEADERS`. Only `http/protobuf` is supported, and the
  default timeout is 500 ms. Export is best effort and never changes the exit
  code.
- **Disarm switches win over `GRIM_TELEMETRY`:** `OTEL_SDK_DISABLED`,
  `DO_NOT_TRACK`, and `GRIM_OFFLINE` / `--offline`. `grim hook run` never
  exports.
- `GRIM_TELEMETRY_REPOSITORY` adds up to four redacted `vcs.*` resource keys
  from the project's `origin` remote (project scope only, never a repository
  that is the home directory).

Each armed command sends one `grim.command` event, plus one
`grim.artifact.changed` event per artifact that `install`, `update` or `add`
changed. Exact attribute lists, and what is never collected, are tier-3 detail:
[fields][fields] and [never collected][never-collected].

## Prove what is sent

At debug level grim logs the exact payload before sending. It prints even when
no endpoint is set and nothing will be sent, and no header value ever appears:

```sh
GRIM_TELEMETRY=1 GRIM_LOG=debug grim status 2>&1 | grep 'telemetry payload'
```

To prove it is off, unset `GRIM_TELEMETRY` (or set a disarm switch) and confirm
no `$GRIM_HOME/telemetry/` directory is created: an unarmed run writes nothing.

## Installation id

`grim.installation.id` is a random UUIDv4 at
`$GRIM_HOME/telemetry/installation-id`, created on the first **armed** run
only. Delete the file to rotate it; the next armed run makes a new one. Machines
or containers that share one `GRIM_HOME` share the id. Walkthrough of a
collector, header scoping and limits: [telemetry][telemetry].

[telemetry]: https://grimoire.rs/telemetry.html
[stability]: https://grimoire.rs/stability.html#unstable
[fields]: https://grimoire.rs/telemetry.html#fields
[never-collected]: https://grimoire.rs/telemetry.html#never-collected
