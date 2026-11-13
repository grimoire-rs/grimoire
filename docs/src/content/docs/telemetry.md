---
title: "Telemetry"
description: "Opt-in anonymous usage telemetry: what grim sends to your OpenTelemetry collector, how to switch it on, and how to audit every byte."
---
<!-- doc_type: reference -->

Grimoire can send one usage record per command to an [OpenTelemetry][otel]
collector that you operate. It is off by default. Nothing is written or sent
until `GRIM_TELEMETRY` or a [`GRIM_CONFIG` file](#config-file) arms it.
Arming writes the [installation ID](#installation-id) file. No request goes
out until an endpoint names a collector.

There is no default endpoint and no maintainer-run collector. Every record
goes to the address you configure, and nowhere else. The whole surface is
[Unstable until 1.0][stability-unstable].

## What is sent {#what-is-sent}

An armed run sends one [OTLP][otlp] HTTP request with a protobuf body to
`/v1/logs`. The request holds the events below, and nothing else:

- One `grim.command` event describing the invocation.
- One `grim.artifact.changed` event per artifact that `grim install`,
  `grim update` or `grim add` changed. An unchanged row sends no event.

The request also carries one resource that describes the installation. The
[field table](#fields) lists every key.

Export is best effort. It never changes the exit code, and it prints nothing
at the default log level. A refused connection, a 5xx answer or a timeout is
visible only at `GRIM_LOG=debug`.

| Invocation | Telemetry |
|---|---|
| A CLI command that runs to its end, including one that fails | One request |
| `grim mcp`, `grim tui` | One `grim.command` event when the session ends. No scope, clients or artifact events |
| A hook firing (the `grim-hook` launcher) | Never exports. The launcher is a separate program with no telemetry code |
| Bare `grim`, `--help`, `--version`, a usage error | No request |
| Every other CLI command | A `grim.command` event only |

## Switch it on {#enable}

Telemetry is armed when `GRIM_TELEMETRY` is truthy and no opt-out applies.
Truthy means `1`, `true`, `yes` or `on`, in any case. A [`GRIM_CONFIG`
file](#config-file) with `enabled = true` arms it too.

A blank `GRIM_TELEMETRY` counts as unset and defers to those files. Any other
value that is not truthy, such as `0` or `false`, is an explicit off. It also
disarms a file.

Setting `OTEL_*` variables alone never arms anything. Without
`GRIM_TELEMETRY` or an enabling file grim makes no telemetry call, even when
every `OTEL_*` variable is present in the environment.

```sh
export GRIM_TELEMETRY=1
export OTEL_EXPORTER_OTLP_ENDPOINT=https://collector.example.com:4318
grim install
```

### Switches {#enable-switches}

| Variable | Effect |
|---|---|
| `GRIM_TELEMETRY` | Arms telemetry. Unset or blank defers to the [`GRIM_CONFIG` files](#config-file). Any other non-truthy value is off. |
| `GRIM_TELEMETRY_REPOSITORY` | Adds up to four `vcs.*` resource keys. Needs telemetry armed, by either source. Project scope only, and never for a repository that is your home directory. Same truthy values. |
| `OTEL_SDK_DISABLED` | A truthy value disarms telemetry. It wins over `GRIM_TELEMETRY` and over a file. |
| `DO_NOT_TRACK` | A truthy value disarms telemetry. It wins over `GRIM_TELEMETRY` and over a file. See [Console Do Not Track][dnt]. |
| `GRIM_OFFLINE`, `--offline` | Either one disarms telemetry, like every other network call. |

### Endpoint and headers {#enable-endpoint}

| Variable | Effect |
|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Base URL. Grim appends `/v1/logs`. A path is kept, so `https://h/otlp/` becomes `https://h/otlp/v1/logs`. |
| `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` | Full URL, used as written. It wins over the base endpoint. |
| `OTEL_EXPORTER_OTLP_HEADERS` | Comma-separated `name=value` pairs, sent only to the base endpoint. |
| `OTEL_EXPORTER_OTLP_LOGS_HEADERS` | Same format. Sent to either endpoint, and it replaces the generic headers. |
| `OTEL_EXPORTER_OTLP_TIMEOUT`, `OTEL_EXPORTER_OTLP_LOGS_TIMEOUT` | Milliseconds, an integer above 0. The logs variable wins. Anything else falls back to the default of 500. |
| `OTEL_EXPORTER_OTLP_PROTOCOL`, `OTEL_EXPORTER_OTLP_LOGS_PROTOCOL` | Only `http/protobuf` is supported. Unset also works. Any other value skips the export. |
| `OTEL_SERVICE_NAME` | Replaces `service.name`. |
| `OTEL_RESOURCE_ATTRIBUTES` | Comma-separated `key=value` pairs added to the resource. Values are percent-decoded. A pair overrides the key grim derived. |

Rules that follow from the table:

- **An endpoint variable is required.** Grim has no default endpoint, not even
  the `localhost:4318` of the OTLP specification. With neither variable set,
  an armed run sends nothing.
- **The URL needs the `http` or `https` scheme and no userinfo.** A URL such
  as `https://user:secret@host` is refused. Put credentials in the headers
  variables.
- **Header values are percent-decoded** and split at the first `=`, so
  `authorization=Bearer%20abc` sends `authorization: Bearer abc`. A malformed
  pair is dropped, and so is a pair named `host`, `content-length`,
  `content-type` or `transfer-encoding`. For a duplicate name the last pair
  wins.
- **The timeout covers connect and the whole request.** Grim does not retry
  and does not follow redirects.
- **A proxy is honoured** when the standard proxy variables (`HTTP_PROXY`,
  `HTTPS_PROXY`, `NO_PROXY`) are set. TLS uses the same [CA roots][ca-roots]
  as registry calls.

### Header scoping {#enable-headers}

The OTLP specification applies `OTEL_EXPORTER_OTLP_HEADERS` to every signal,
including a signal with its own endpoint. Grim deviates on purpose. The
generic headers go only to the base endpoint. A request to
`OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` carries `OTEL_EXPORTER_OTLP_LOGS_HEADERS`
and nothing else.

An ambient `OTEL_EXPORTER_OTLP_HEADERS` reaches the base endpoint of any
environment that arms telemetry. A CI runner or a shared shell profile that
exports the headers and arms telemetry sends that credential on every command.
An endpoint set by a [`GRIM_CONFIG` file](#config-file) never receives ambient
`OTEL_*` headers. Set the headers only in shells you control, and prefer an
`https://` endpoint.

### Unsupported variables {#enable-unsupported}

Grim sends an uncompressed body and uses the system trust store plus its
built-in roots. These variables have no effect:

- `OTEL_EXPORTER_OTLP_COMPRESSION`, `OTEL_EXPORTER_OTLP_LOGS_COMPRESSION`. A
  value of `gzip` still sends the body uncompressed.
- `OTEL_EXPORTER_OTLP_CERTIFICATE` and the `OTEL_EXPORTER_OTLP_CLIENT_*`
  variables, with their `_LOGS_` forms.

### Limits {#enable-limits}

A hostname whose DNS resolver hangs can hold a command past the timeout. The
runtime waits for the blocking resolver thread before it exits. Use an IP
address, or a resolver that answers, for a collector that must never delay
a command.

With `GRIM_TELEMETRY_REPOSITORY` set, up to two `git` calls run before the
export. Each is cut off after 1 second.

The export is lost when the process is killed or interrupted, for example by
Ctrl-C.

## Configure with a file {#config-file}

A tool manager composes one environment for many tools. Any `OTEL_*` variable
set for grim there reaches every OpenTelemetry-aware process in that
environment. It also hands them grim's token.

`GRIM_CONFIG` avoids that. Only grim reads it. It names TOML files that arm
telemetry, name the collector and carry the headers, so an organisation can
ship one file and set one variable. Everything below is [Unstable until
1.0][stability-unstable].

### The variable {#config-file-variable}

`GRIM_CONFIG` holds a list of file paths. The separator is `:` on Linux and
macOS and `;` on Windows. A blank value counts as unset, and empty entries are
skipped.

- Every entry must be an absolute path. A relative entry is refused and never
  read. There is no `~` or `$VAR` expansion.
- Every entry must be a regular file. A symlink to one is followed. A
  directory, FIFO or device is refused before it is opened.
- A file larger than 8 MiB is refused, the same cap as `grimoire.toml`.
- Files are read in list order. A later file wins over an earlier one.
- Only the `[telemetry]` table is read. Any other key, such as `[options]` or
  `[[registries]]`, is ignored with one warning. Artifact declarations,
  consent records and registry credentials never come from these files.

`grim config` neither reads nor writes these files. Grim reads no system or
user config file on its own.

### Keys {#config-file-keys}

| Key | Type | Meaning |
|---|---|---|
| `enabled` | bool | `true` arms telemetry like `GRIM_TELEMETRY=1`. |
| `endpoint` | string | Base URL. Grim appends `/v1/logs`. Needs `http` or `https`, a host and no userinfo. |
| `headers` | table of strings | OTLP headers, sent only to this `endpoint`. Values are not percent-decoded. An invalid or reserved name, or an invalid value, makes the file unusable ([warning](#config-file-warnings)). |
| `timeout` | integer | Milliseconds, above 0. Default 500. Used only with a file `endpoint`. |
| `protocol` | string | Only `http/protobuf`, in any case. |
| `service_name` | string | Replaces `service.name`. An empty string is ignored. |
| `resource_attributes` | table of strings | Keys added to the resource. |

Across files, a later scalar replaces an earlier one. `headers` merge per name,
ignoring case. `resource_attributes` merge per exact key. The later file wins
in both.

### How arming is decided {#config-file-arming}

Grim decides once per command, before any telemetry code runs. The first rule
that applies wins:

1. `DO_NOT_TRACK` or `OTEL_SDK_DISABLED` is truthy, `GRIM_OFFLINE` is truthy,
   or `--offline` is given. Telemetry is off. No file is opened and nothing
   warns.
2. `GRIM_TELEMETRY` is set to a non-blank value that is not truthy. Telemetry
   is off and no file is opened. A blank value counts as unset.
3. Grim reads the files. An unusable file turns telemetry off for this run,
   even with `GRIM_TELEMETRY=1`, and prints one [warning](#config-file-warnings).
4. Telemetry is armed when `GRIM_TELEMETRY` is truthy or the merged `enabled`
   is `true`. `GRIM_TELEMETRY=1` arms even when a file says `enabled = false`.

`GRIM_TELEMETRY_REPOSITORY` applies whenever telemetry is armed, by either
source. It stays environment-only.

### The destination {#config-file-destination}

The destination is one unit, chosen by whether a file sets `endpoint`.

With a file `endpoint`, grim reads no `OTEL_EXPORTER_OTLP_*` variable: not the
endpoints, headers, protocol or timeout. Ambient values never change the
request. The headers are the merged file headers, then
`GRIM_TELEMETRY_OTLP_HEADERS`. The protocol is `http/protobuf` and the timeout
is the file value or 500 ms.

With no file `endpoint`, the [`OTEL_*` rules](#enable-endpoint) apply as
before. Grim never sends the file headers or `GRIM_TELEMETRY_OTLP_HEADERS`
there. A credential grim owns travels only to an endpoint grim owns. A
`timeout` without a file `endpoint` is unused and silent.

The resource is built the same way for both destinations. Later entries
override earlier ones:

1. The keys grim derives.
2. `OTEL_RESOURCE_ATTRIBUTES`.
3. `OTEL_SERVICE_NAME`.
4. The file `resource_attributes`.
5. The file `service_name`.

A `service.name` attribute in a file therefore beats an ambient
`OTEL_SERVICE_NAME`.

### Headers from the environment {#config-file-headers}

`GRIM_TELEMETRY_OTLP_HEADERS` holds comma-separated `name=value` pairs. Values
are percent-decoded, as for `OTEL_EXPORTER_OTLP_HEADERS`. A pair replaces a file
header of the same name, ignoring case. It is sent only with a file `endpoint`.

### Warnings {#config-file-warnings}

Warnings go to stderr at the default log level. They never change the exit
code or stdout. No warning, log line or error ever holds a header value, file
content or endpoint userinfo.

| Warning | When |
|---|---|
| `GRIM_CONFIG entry '<entry>' is not an absolute path` | A relative entry. |
| `cannot read GRIM_CONFIG file '<path>': <os error>` | A missing or unreadable file. |
| `GRIM_CONFIG file '<path>' is not a regular file` | A directory, FIFO or device. |
| `GRIM_CONFIG file '<path>' is larger than <N> bytes` | A file over the cap. |
| `GRIM_CONFIG file '<path>' is not valid TOML (line L, column C)` | A parse error. |
| `GRIM_CONFIG file '<path>' has an unusable value for '<key>'` | A wrong type or value for a known key. |
| `ignored GRIM_CONFIG keys in '<path>': <keys>` | Unknown keys, when every file loaded. |
| `telemetry headers from GRIM_CONFIG or GRIM_TELEMETRY_OTLP_HEADERS are not sent: no GRIM_CONFIG file sets an endpoint` | Grim headers with no file endpoint, in an armed run. |

The first six end with `; telemetry is off for this run`. A listed file that
does not exist warns on every command until you fix it. That is deliberate: a
mistyped organisation path must not drop its destination silently.

### Fleet example {#config-file-fleet}

An [ocx][ocx] patch package can ship the file and set the variable. The value
must be absolute, and `"visibility": "public"` lets a shell that runs `grim`
directly see it. The entry overwrites any `GRIM_CONFIG` already in the composed
environment.

<!-- doc-norun: ocx patch metadata, not a grim command -->
```json
{"key": "GRIM_CONFIG", "type": "constant", "value": "${installPath}/telemetry.toml", "visibility": "public"}
```
<!-- /doc-norun -->

The package ships `telemetry.toml`:

<!-- doc-norun: file shipped by the organisation, with a placeholder token -->
```toml
[telemetry]
enabled = true
endpoint = "https://otel.example.com:4318"

[telemetry.headers]
authorization = "Bearer <token>"
```
<!-- /doc-norun -->

A user who wants their own collector lists a second file after it. Its
`endpoint` replaces the organisation's. Headers merge per name, so the
organisation's headers go to that collector too unless the user's file names
each of them. To keep them away, list only your own file in `GRIM_CONFIG`.

```sh
export GRIM_CONFIG="$ORG_FILE:$HOME/.config/grim/telemetry.toml"
```

A user who wants telemetry off sets `GRIM_TELEMETRY=0` or `DO_NOT_TRACK=1`.

### Who can set it {#config-file-trust}

A repository can carry its own environment: an `.envrc`, a trusted
`.mise.toml`, a devcontainer `containerEnv` or CI `variables:`. Such an
environment can set `GRIM_CONFIG` to a file the repository ships. That arms
telemetry and chooses the endpoint.

This is the same exposure as `GRIM_TELEMETRY` plus an `OTEL_*` endpoint set by
that environment. It collects command names and artifact references. It can
also receive `GRIM_TELEMETRY_OTLP_HEADERS`, or an organisation file's headers
when the repository lists its file after the organisation's. Anything that
injects a repository's environment can already run code.

Two points narrow it. Ambient `OTEL_*` headers never reach a `GRIM_CONFIG`
endpoint. A relative entry is refused, so a committed file cannot become a
layer with no environment at all.

Prefer the file form for tokens, and protect the file with permissions. Every
child process grim starts, such as `git`, `gh`, `glab` and MCP servers,
inherits `GRIM_TELEMETRY_OTLP_HEADERS`.

## Fields {#fields}

Every key below is a name from the OpenTelemetry semantic conventions where
one exists, and a `grim.*` key otherwise. A key that is absent means "does not
apply to this command". It never means false.

### Resource {#fields-resource}

| Key | Type | Presence | Example |
|---|---|---|---|
| `service.name` | string | always | `grim` |
| `service.version` | string | always | `0.15.0` |
| `os.type` | string | always | `linux`, `darwin`, `windows` |
| `host.arch` | string | always | `amd64`, `arm64` |
| `grim.installation.id` | string | when the [installation ID](#installation-id) could be read or created | `3f0c9a52-5d2e-4c1b-8a3e-7b1d2f4a6c90` |
| `vcs.repository.url.full` | string | only with `GRIM_TELEMETRY_REPOSITORY`, project scope, and an `origin` remote | `https://github.com/acme/repo` |
| `vcs.owner.name` | string | with `vcs.repository.url.full`, unless the remote path has one segment, as in `https://host/repo` | `acme` |
| `vcs.repository.name` | string | with `vcs.repository.url.full` | `repo` |
| `vcs.provider.name` | string | with `vcs.repository.url.full`, when the host contains `github`, `gitlab`, `bitbucket` or `gitea`, or is `codeberg.org` | `github` |

`OTEL_RESOURCE_ATTRIBUTES` and `OTEL_SERVICE_NAME` can add or replace keys.
There is no `telemetry.sdk.*`, `host.name`, `process.*` or `user.*` key.

### The grim.command event {#fields-command}

| Key | Type | Presence | Example |
|---|---|---|---|
| `grim.command.name` | string | always | `install` |
| `grim.command.subcommand` | string | only for `config`, `export` and `hook` | `registry add` |
| `grim.command.duration` | double, seconds | always | `0.412` |
| `process.exit.code` | int | always | `0` |
| `error.type` | string | only when the exit code is not 0 | `stale-lock` |
| `grim.ci` | bool | always | `false` |
| `grim.scope` | string | when the command resolved its scope. Never for `mcp` and `tui` | `project` |
| `grim.clients` | string array | `install`, `update` and `add` only. Not sent for `add --no-install` | `["claude"]` |
| `grim.clients.detected` | string array | `install`, `update` and `add` only. Not sent for `add --no-install` | `["claude","codex"]` |

`grim.command.name` is the canonical command name, so an alias reports as the
command it stands for. `error.type` is the [error reason][json-interface-error]
when one applies, otherwise the exit-code name. A closed stdout pipe reports
`process.exit.code` as `0`, matching the exit status.

`grim.ci` is true when `CI`, `GITHUB_ACTIONS` or `GITLAB_CI` is truthy. Only
those names are read, never other variables. Client arrays list client names
in a fixed order.

### The grim.artifact.changed event {#fields-artifact}

| Key | Type | Presence | Example |
|---|---|---|---|
| `grim.artifact.kind` | string | always | `skill`, `rule`, `agent`, `bundle`, `mcp`, `hook`, `settings` |
| `grim.artifact.action` | string | always | `installed`, `updated`, `removed`, `added` |
| `grim.artifact.ref` | string | omitted for a local path source | `ghcr.io/acme/review:1` |

`installed` and `updated` describe what grim wrote on this machine, for
`grim install` and `grim update` alike. A pin that moved in the lock while
every selected client declined the kind writes nothing and sends no event.
`removed` is an artifact `grim update` pruned after it left the lock.

The reference is `registry/repository`, plus `:tag` when the declaration
carries one. When several declarations name the same repository, the tag comes
from the first one. Skills come first, then rules, agents, bundles, MCP servers
and hooks, each in name order. It never holds a digest, userinfo or a local path. A bundle
install sends one event per changed member. `grim add` of a bundle sends one
event of kind `bundle`.

## Never collected {#never-collected}

Grim does not read these values for telemetry, and no field above carries
them:

- Git branch name, commit SHA and commit author.
- Host name and user name.
- Working directory, `GRIM_HOME` path and any other filesystem path.
- Command-line arguments, including config paths and search terms.
- Registry credentials, tokens and the values of your headers variables.
- Artifact digests and the contents of any artifact or config file.
- Repository URL userinfo, query string and fragment.

A repository URL passes through redaction before it leaves the machine.
Credentials, query and fragment are removed, the host and path are lowercased,
and ssh forms become `https://`. A local path or a `file://` remote produces no
`vcs.*` keys at all.

## Installation ID {#installation-id}

`grim.installation.id` is a random version 4 UUID. It tells your collector
how many installations exist and whether one repeats. It is derived from
nothing about the machine or the user.

The first armed run writes it to `$GRIM_HOME/telemetry/installation-id`. Later
runs reuse it. An unarmed run never creates the file or the directory.

- Delete the file to rotate the ID. The next armed run creates a new one.
- A file that does not hold a valid UUID is replaced.
- Machines or containers that share one `GRIM_HOME` volume share one
  installation ID.
- With a relative or unwritable `GRIM_HOME`, the key is left out and the
  export continues.

## Audit what is sent {#audit}

At `GRIM_LOG=debug` grim logs the exact payload before it sends it. The line
comes from the `grim::telemetry` log target, and its message is
`telemetry payload` followed by one JSON object that runs to the end of the
line:

```json
{"events":[{"attributes":{"grim.ci":false,"grim.clients":["claude"],"grim.clients.detected":["claude","codex"],"grim.command.duration":0.412,"grim.command.name":"install","grim.scope":"project","process.exit.code":0},"name":"grim.command"},{"attributes":{"grim.artifact.action":"installed","grim.artifact.kind":"skill","grim.artifact.ref":"ghcr.io/acme/review:1"},"name":"grim.artifact.changed"}],"resource":{"grim.installation.id":"3f0c9a52-5d2e-4c1b-8a3e-7b1d2f4a6c90","host.arch":"amd64","os.type":"linux","service.name":"grim","service.version":"0.15.0"}}
```

The JSON and the wire request carry the same keys and values. Events appear in
send order, with `grim.command` first. Grim sorts the keys inside each object
alphabetically. Key order is not part of the contract.

The line is printed whenever telemetry is armed, including when no endpoint
is set and nothing will be sent. Further debug lines name a skipped export, a
failed export, a missing installation ID or a dropped malformed pair. A
dropped pair is named by its key only. A failed export shows the endpoint
with userinfo and query removed. No line ever shows a header value.

```sh
GRIM_TELEMETRY=1 GRIM_LOG=debug grim status 2>&1 | grep 'telemetry payload'
```

## Collector recipe {#collector-recipe}

A collector can turn the events into metrics. This `otelcol-contrib`
configuration uses the [count connector][count-connector] for
`grim.invocations`, `grim.errors` and `grim.artifact.changes`. It uses the
[signal to metrics connector][signaltometrics-connector] for the
`grim.invocation.duration` histogram, in seconds.

<!-- doc-norun: collector configuration for a separate service, not a grim command -->
```yaml
receivers:
  otlp:
    protocols:
      http:
        endpoint: 0.0.0.0:4318

connectors:
  count:
    logs:
      grim.invocations:
        description: grim commands run
        conditions:
          - log.event_name == "grim.command"
        attributes:
          - key: grim.command.name
      grim.errors:
        description: grim commands that failed
        conditions:
          - log.attributes["error.type"] != nil
        attributes:
          - key: error.type
      grim.artifact.changes:
        description: artifacts changed
        conditions:
          - log.event_name == "grim.artifact.changed"
        attributes:
          - key: grim.artifact.kind
          - key: grim.artifact.action
  signal_to_metrics:
    logs:
      - name: grim.invocation.duration
        description: grim command duration in seconds
        conditions:
          - log.event_name == "grim.command"
        attributes:
          - key: grim.command.name
        histogram:
          buckets: [0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, 60]
          value: Double(log.attributes["grim.command.duration"])

exporters:
  debug:
    verbosity: basic

service:
  pipelines:
    logs:
      receivers: [otlp]
      exporters: [count, signal_to_metrics]
    metrics:
      receivers: [count, signal_to_metrics]
      exporters: [debug]
```
<!-- /doc-norun -->

Replace the `debug` exporter with the exporter for your metrics backend. Point
grim at the receiver with `OTEL_EXPORTER_OTLP_ENDPOINT=http://<collector>:4318`.

Both connectors are alpha upstream, so option names can change between
collector releases. Check the connector READMEs for the release you run. The
count connector drops the default `log.record.count` metric once a `logs`
section defines its own.

## Stability {#stability}

Everything on this page is Unstable until 1.0. That covers the switches,
`GRIM_CONFIG`, the `[telemetry]` table of its files and
`GRIM_TELEMETRY_OTLP_HEADERS`. It also covers the event names, every attribute
and resource key, the debug line and the installation ID file.
Keys can be added or renamed in any minor release. See
[Unstable][stability-unstable].

<!-- internal -->
[stability-unstable]: ./stability.md#unstable
[ca-roots]: ./configuration.md#ca-roots
[json-interface-error]: ./json-interface.md#error-document

<!-- external -->
[otel]: https://opentelemetry.io/
[ocx]: https://ocx.sh
[otlp]: https://opentelemetry.io/docs/specs/otlp/
[dnt]: https://consoledonottrack.com/
[count-connector]: https://github.com/open-telemetry/opentelemetry-collector-contrib/tree/main/connector/countconnector
[signaltometrics-connector]: https://github.com/open-telemetry/opentelemetry-collector-contrib/tree/main/connector/signaltometricsconnector
