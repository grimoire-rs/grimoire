---
title: "Telemetry"
description: "Opt-in anonymous usage telemetry: what grim sends to your OpenTelemetry collector, how to switch it on, and how to audit every byte."
---
<!-- doc_type: reference -->

Grimoire can send one usage record per command to an [OpenTelemetry][otel]
collector that you operate. It is off by default. Nothing is written or sent
until `GRIM_TELEMETRY` is set. Arming writes the
[installation ID](#installation-id) file. No request goes out until an
endpoint variable points at a collector.

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
| `grim hook run` | Never exports, even when armed |
| Bare `grim`, `--help`, `--version`, a usage error | No request |
| Every other CLI command | A `grim.command` event only |

## Switch it on {#enable}

Telemetry is armed when `GRIM_TELEMETRY` is truthy and no opt-out applies.
Truthy means `1`, `true`, `yes` or `on`, in any case. An empty value, `0` and
`false` leave it off.

Setting `OTEL_*` variables alone never arms anything. Without
`GRIM_TELEMETRY` grim makes no telemetry call, even when every `OTEL_*`
variable is present in the environment.

```sh
export GRIM_TELEMETRY=1
export OTEL_EXPORTER_OTLP_ENDPOINT=https://collector.example.com:4318
grim install
```

### Switches {#enable-switches}

| Variable | Effect |
|---|---|
| `GRIM_TELEMETRY` | Arms telemetry. Unset means off. |
| `GRIM_TELEMETRY_REPOSITORY` | Adds up to four `vcs.*` resource keys. Needs `GRIM_TELEMETRY`. Project scope only, and never for a repository that is your home directory. Same truthy values. |
| `OTEL_SDK_DISABLED` | A truthy value disarms telemetry. It wins over `GRIM_TELEMETRY`. |
| `DO_NOT_TRACK` | A truthy value disarms telemetry. It wins over `GRIM_TELEMETRY`. See [Console Do Not Track][dnt]. |
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

An ambient `OTEL_EXPORTER_OTLP_HEADERS` still reaches the base endpoint of any
environment that also sets `GRIM_TELEMETRY`. A CI runner or a shared shell
profile that exports both sends that credential to that endpoint on every
command. Set the headers only in shells you control, and prefer an `https://`
endpoint.

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
| `grim.artifact.kind` | string | always | `skill`, `rule`, `agent`, `bundle`, `mcp`, `hook` |
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

Everything on this page is Unstable until 1.0: both switches, the event names,
every attribute and resource key, the debug line and the installation ID file.
Keys can be added or renamed in any minor release. See
[Unstable][stability-unstable].

<!-- internal -->
[stability-unstable]: ./stability.md#unstable
[ca-roots]: ./configuration.md#ca-roots
[json-interface-error]: ./json-interface.md#error-document

<!-- external -->
[otel]: https://opentelemetry.io/
[otlp]: https://opentelemetry.io/docs/specs/otlp/
[dnt]: https://consoledonottrack.com/
[count-connector]: https://github.com/open-telemetry/opentelemetry-collector-contrib/tree/main/connector/countconnector
[signaltometrics-connector]: https://github.com/open-telemetry/opentelemetry-collector-contrib/tree/main/connector/signaltometricsconnector
