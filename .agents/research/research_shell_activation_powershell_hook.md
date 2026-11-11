# Research: Shell activation — PowerShell 5.1 and 7 prompt hook

## Metadata

**Date:** 2026-11-11
**Domain:** cli
**Triggered by:** /hex-architect shell-activation
**Expires:** 2027-05-11

Method: tool hook code was fetched from the raw upstream files on 2026-11-11 (default branches, so "main" at that date) and quoted verbatim below. Search-engine summaries were used only to find sources; every claim carries a URL. `[verify]` marks anything I could not confirm from a fetched source. Sources older than ~May 2025 are flagged `(OLD)` in the Sources table.

## Direct Answer

No surveyed tool has a "idle prompt costs zero processes plus a background refresh" design on PowerShell. The proven building blocks are:

- **Prompt hook:** wrap the global `prompt` function, save the previous one, call it last, and guard against double install with a global sentinel. zoxide and mise do exactly this. starship and oh-my-posh instead replace `prompt` outright and do not chain, so they must initialise before the chaining tools.
- **`LocationChangedAction`:** works on PowerShell 7 only (direnv requires 7.2; mise skips it below 7). It is a supplement for cd-triggered work, not a substitute for a prompt hook on 5.1.
- **Cheap idle check:** mise's own source says an idle hook-env spawn on Windows costs "~22ms, almost entirely CreateProcess" and it added a PowerShell-side skip to avoid it. That is the closest precedent for requirement (2).
- **Detached background process:** the console-flash problem is real and unfixed in `powershell.exe -WindowStyle Hidden` (PowerShell issue 3028). The flash-free primitive is the Win32 `DETACHED_PROCESS` flag, which Rust can set; PowerShell 5.1 has no first-class way to set it. The most robust option is to have `grim` itself spawn the detached worker.
- **Completions:** `clap_complete`'s PowerShell output is a `Register-ArgumentCompleter -Native` block that begins with `using namespace` lines. Those lines are the one thing that breaks when the output is embedded in a larger evaluated string.

## 1. Prompt hook mechanics

### Hook code per tool (verbatim from cited source)

**zoxide** (`templates/powershell.txt`, [source](https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/templates/powershell.txt)). Chains the previous prompt, guarded by `$__zoxide_hooked`:

```powershell
$global:__zoxide_hooked = (Microsoft.PowerShell.Utility\Get-Variable __zoxide_hooked -ErrorAction Ignore -ValueOnly)
if ($global:__zoxide_hooked -ne 1) {
    $global:__zoxide_hooked = 1
    $global:__zoxide_prompt_old = $function:prompt

    function global:prompt {
        if ($null -ne $__zoxide_prompt_old) {
            & $__zoxide_prompt_old
        }
        $exitCode = Microsoft.PowerShell.Utility\Get-Variable LASTEXITCODE -Scope Global -ErrorAction Ignore -ValueOnly
        $null = __zoxide_hook
        $global:LASTEXITCODE = $exitCode
    }
}
```

Its `pwd` hook variant compares `__zoxide_pwd` to `$global:__zoxide_oldpwd` so it only spawns `zoxide add` when the directory changed. Install line: `Invoke-Expression (& { (zoxide init powershell | Out-String) })`, "at the end of your config file" ([README](https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/README.md)).

**mise** (`src/shell/pwsh.rs`, [source](https://raw.githubusercontent.com/jdx/mise/main/src/shell/pwsh.rs)). Chains the previous prompt and, on PowerShell 7, also combines a `LocationChangedAction` delegate:

```powershell
function __enable_mise_prompt {
    if (-not (Test-Path variable:global:__mise_pwsh_previous_prompt_function)){
        $Global:__mise_pwsh_previous_prompt_function=$function:prompt
        function global:prompt {
            # Read first: every command below sets $?, and hook-env is a native
            # command. _mise_hook puts $LASTEXITCODE back; $? cannot be assigned,
            # but an ignored error makes it false again for the prompt it wraps.
            $previousSucceeded = $?
            if (Test-Path -Path Function:\_mise_hook){
                ...
                if ( $null -eq $handled -or $handled.Path -ne $PWD.Path -or $handled.MiseEnv -ne ... ) {
                    _mise_hook
                }
            }
            if (-not $previousSucceeded) { Write-Error '' -ErrorAction Ignore }
            & $__mise_pwsh_previous_prompt_function
        }
    }
}
```

and (same file):

```powershell
$__mise_pwsh_previous_chpwd_function=$ExecutionContext.SessionState.InvokeCommand.LocationChangedAction;
if ($__mise_pwsh_previous_chpwd_function) {
    $ExecutionContext.SessionState.InvokeCommand.LocationChangedAction = [Delegate]::Combine($__mise_pwsh_previous_chpwd_function, $_mise_chpwd_hook)
}
else {
    $ExecutionContext.SessionState.InvokeCommand.LocationChangedAction = $_mise_chpwd_hook
}
```

Below 7 the chpwd branch only prints a warning and returns (`if ($PSVersionTable.PSVersion.Major -lt 7) { ... Write-Warning "mise: chpwd functionality requires PowerShell version 7 or higher..." ; return }`). The wrapper's `_mise_hook` runs `& '{exe}' hook-env ... | Out-String` and `Invoke-Expression`s the result. Note the file is large; only the cited excerpts are quoted.

**direnv** (`internal/cmd/shell_pwsh.go`, [source](https://raw.githubusercontent.com/direnv/direnv/master/internal/cmd/shell_pwsh.go)). `LocationChangedAction` only, PowerShell 7.2+, no prompt hook:

```powershell
if ($PSVersionTable.PSVersion.Major -lt 7 -or ($PSVersionTable.PSVersion.Major -eq 7 -and $PSVersionTable.PSVersion.Minor -lt 2)) {
    throw "direnv: PowerShell version $($PSVersionTable.PSVersion) does not meet the minimum required version 7.2!"
}
$hook = [EventHandler[LocationChangedEventArgs]] {
  param([object] $source, [LocationChangedEventArgs] $eventArgs)
  end {
    $export = (& '{{.PwshSelfPath}}' export pwsh) -join [Environment]::NewLine;
    if ($export) {
      Invoke-Expression -Command $export;
    }
  }
};
$currentAction = $ExecutionContext.SessionState.InvokeCommand.LocationChangedAction;
if ($currentAction) {
  $ExecutionContext.SessionState.InvokeCommand.LocationChangedAction = [Delegate]::Combine($currentAction, $hook);
}
else {
  $ExecutionContext.SessionState.InvokeCommand.LocationChangedAction = $hook;
};
```

**starship** (`src/init/starship.ps1`, [source](https://raw.githubusercontent.com/starship/starship/master/src/init/starship.ps1)). Defines `function global:prompt` inside a `New-Module` and does **not** call any earlier prompt. It spawns `starship prompt` on every prompt through `System.Diagnostics.Process`:

```powershell
$startInfo = New-Object System.Diagnostics.ProcessStartInfo -ArgumentList $Executable -Property @{
    StandardOutputEncoding = [System.Text.Encoding]::UTF8;
    RedirectStandardOutput = $true;
    RedirectStandardError = $true;
    CreateNoWindow = $true;
    UseShellExecute = $false;
};
```

It saves/restores `$LASTEXITCODE` and replays `$?` ("`Write-Error '' -ErrorAction 'Ignore'`" as the last statement), calls `Set-PSReadLineOption -ExtraPromptLineCount`, and overrides the PSReadLine continuation prompt and vi-mode handler. It branches on `$PSVersionTable.PSVersion.Major -gt 5` for `STARSHIP_SHELL` and on `.ArgumentList.Add` existence for argument quoting (5.1 uses a manual C-runtime quoting path).

**oh-my-posh** (`src/shell/scripts/omp.ps1`, [source](https://raw.githubusercontent.com/JanDeDobbeleer/oh-my-posh/main/src/shell/scripts/omp.ps1)). Captures `$script:OriginalPromptFunction = $Function:prompt` (line 50) but only to restore it on removal (`if ($Function:prompt -eq $promptFunction) { $Function:prompt = $script:OriginalPromptFunction }`, ~line 1378); its replacement `prompt` does not chain. It is the one tool that uses an engine event:

```powershell
$script:StreamingOnIdleJob = Register-EngineEvent -SourceIdentifier PowerShell.OnIdle -Action {
    $s = $global:_ompStreamingState
    ...
    [Microsoft.PowerShell.PSConsoleReadLine]::InvokePrompt()
```

Its own comment explains why: "OnIdle is also the ONLY async consumer of streamed records. It is an engine-generated event on the pipeline thread: consuming records here instead of in a DataAdded subscription means no PSEvent is ever raised from a background thread. Cross-thread event delivery can re-enter the engine's pulse pipeline and crash the host with InvalidPipelineStateException ... under rapid prompt cycles."

**fnm** (`src/shell/powershell.rs`, [source](https://raw.githubusercontent.com/Schniz/fnm/master/src/shell/powershell.rs)). No prompt or event hook. It shadows `cd`:

```powershell
function global:Set-LocationWithFnm { param($path); if ($path -eq $null) {Set-Location} else {Set-Location $path}; Set-FnmOnLoad }
Set-Alias -Option AllScope -Scope global cd Set-LocationWithFnm
```

and guards with `(Test-Path .nvmrc) -Or (Test-Path .node-version)` so no process is spawned unless a version file exists. It only fires on `cd`, not on other directory changes.

**atuin** (`crates/atuin/src/shell/atuin.ps1`, [source](https://raw.githubusercontent.com/atuinsh/atuin/main/crates/atuin/src/shell/atuin.ps1)). No prompt hook. It defines a `PSConsoleHostReadLine` override and binds keys (`Set-PSReadLineKeyHandler -Chord "Ctrl+r" ...`, `"UpArrow"`), and requires PSReadLine (`Write-Error "Atuin requires the PSReadLine module to be installed."`). It only reads the prompt (`& $Function:prompt | Out-String | Measure-Object -Line`) to compute an offset.

### Per-tool comparison

| Tool | Prompt wrap | Chains old `prompt` | `LocationChangedAction` | Engine event | Min. version gate | Spawns per prompt |
|---|---|---|---|---|---|---|
| zoxide | yes | yes (`$__zoxide_prompt_old`) | no | no | none (`cd -`/`cd +` need 6.1/6.2) | `zoxide add` per prompt (prompt hook) or only on dir change (`--hook pwd`) |
| mise | yes | yes | yes, 7+ only, combined via `[Delegate]::Combine` | no | chpwd needs 7 | skips spawn if chpwd handler already handled this dir (documented ~22 ms CreateProcess saved) |
| direnv | no | n/a | yes, `Delegate::Combine` | no | throws below 7.2 | on dir change only |
| starship | yes | no, replaces | no | no | works on 5.1, branches on version | one `starship prompt` per prompt (required, it draws the prompt) |
| oh-my-posh | yes | no, replaces (saves for restore) | no | `PowerShell.OnIdle` | not checked here | one process per prompt |
| fnm | no | n/a | no (shadows `cd` alias) | no | none | only when a version file exists |
| atuin | no | n/a | no | no | needs PSReadLine | none from prompt |

### Coexistence

- Each chaining tool is idempotent through a sentinel (`__zoxide_hooked`, `__mise_pwsh_previous_prompt_function`). Two chaining tools therefore stack in init order and both keep working (zoxide and mise source above).
- A **non-chaining** tool (starship, oh-my-posh) installed after a chaining one silently drops the earlier wrapper. zoxide's docs place its init at the end of the profile for this reason ([README](https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/README.md)). A third-party module documents the same ordering rule and a second failure mode: oh-my-posh removes and re-adds its dynamic module on each init, and zoxide's one-shot flag means a profile reload can drop zoxide's hook; its workaround is `zoxide init powershell --hook none` plus a `LocationChangedAction` handler ([ScrewCitySoftware.PwshProfile](https://www.powershellgallery.com/packages/ScrewCitySoftware.PwshProfile/0.7.3/Content/Public%5CTools%5CEnable-Zoxide.ps1), third-party, not an upstream issue).
- I did not find a filed upstream issue on zoxide, starship or oh-my-posh for this ordering problem; my search did not reach their issue trackers, so absence is not evidence `[verify]`.
- Every wrapper has to preserve `$LASTEXITCODE` and `$?`; mise and starship both document that `$?` cannot be assigned and replay it with `Write-Error '' -ErrorAction Ignore`. zoxide preserves only `$LASTEXITCODE`.
- Strict mode: mise had a bug class where its init read unassigned globals, which `Set-StrictMode` turns into terminating errors ([mise discussion 5312](https://github.com/jdx/mise/discussions/5312), referenced in its [test](https://raw.githubusercontent.com/jdx/mise/main/e2e-win/activate_strictmode.Tests.ps1)). zoxide's template carries the same fix (comment: "Removing this will cause an unset variable error in StrictMode").

## 2. 5.1 versus 7 differences that affect an emitted script

| Topic | 5.1 | 7 | Source |
|---|---|---|---|
| Ternary `? :`, `&&` / `\|\|` | not available | introduced in 7.0 | [What's new in 7.0](https://learn.microsoft.com/en-us/powershell/scripting/whats-new/what-s-new-in-powershell-70?view=powershell-7.5) |
| `??`, `??=` | not available `[verify]` (page lists no version; I believe 7.0) | available | [about_Operators](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_operators?view=powershell-7.5) |
| `?.` / `?[]` | not available `[verify]` | mainstream in 7.1 (the page says so) | same |
| `&` background operator | not available | available (equals `Start-Job`) | same |
| `$IsWindows` / `$IsLinux` / `$IsMacOS` | **absent**: the 5.1 automatic-variables page contains no `IsWindows` (my grep of the page returned 0 hits) | documented | [5.1](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_automatic_variables?view=powershell-5.1), [7.5](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_automatic_variables?view=powershell-7.5) |
| `$PSStyle` (ANSI) | absent | 7.2+ | [about_ANSI_Terminals](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_ansi_terminals?view=powershell-5.1) |
| `ProcessStartInfo.ArgumentList` | absent (.NET Framework) | present | starship source above |
| `LocationChangedAction` | tools gate it to 7 (mise) / 7.2 (direnv); whether 5.1 has the property is not stated `[verify]` | works | sources above |

Rule for the emitted script: write for the 5.1 grammar (no ternary, no `&&`, no `??`, no `?.`) and use `$PSVersionTable.PSVersion.Major -ge 7` / `$PSEdition` for branching instead of `$IsWindows`. The `$IsWindows` form in 5.1 is read as an unset variable (an error under strict mode); starship and zoxide both branch on `$PSVersionTable.PSVersion` (sources above).

**Native output encoding and `Invoke-Expression (& tool init | Out-String)`.**
- PowerShell decodes native stdout using `[Console]::OutputEncoding`, not `$OutputEncoding`; on Windows PowerShell 5.1 that defaults to the OEM code page, so non-ASCII UTF-8 output can be garbled. Microsoft says `$OutputEncoding` "affects the encoding PowerShell uses to communicate with external programs" ([about_Character_Encoding](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_character_encoding?view=powershell-7.5)); the decoding side comes from community sources and a search summary ([mklement0 gist](https://gist.github.com/mklement0/ef57aea441ea8bd43387a7d7edfc6c19), [PowerShell issue 7233](https://github.com/PowerShell/PowerShell/issues/7233), both not fetched in full `[verify]`).
- zoxide's template shows the accepted fix: it wraps every binary call in `[Console]::OutputEncoding = [System.Text.Utf8Encoding]::new()` with a `finally` restore ([source](https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/templates/powershell.txt)); mise does the same ("sets UTF-8 output encoding"); starship avoids the question by setting `StandardOutputEncoding = UTF8` on a `ProcessStartInfo`.
- Consequence for grim: if the init script is ASCII-only, `Invoke-Expression (& grim shell init powershell | Out-String)` is safe in both editions with no encoding step. Any non-ASCII text (a localised notice, a path) requires the zoxide pattern. Do not rely on UTF-16: the question's premise that native output arrives as UTF-16 is not supported by anything I found; the documented default is the OEM code page.
- `Out-String` matters beyond encoding: without it, piping a native command to `Invoke-Expression` evaluates each output line separately, which breaks multi-line constructs ([clap PR 5848](https://github.com/clap-rs/clap/pull/5848) description, as returned by a fetch summary `[verify]` in exact wording).

**`$PROFILE` paths.**
- PowerShell 7: Windows `$HOME\Documents\PowerShell\Microsoft.PowerShell_profile.ps1`; Linux and macOS `~/.config/powershell/Microsoft.PowerShell_profile.ps1`; VS Code host `Microsoft.VSCode_profile.ps1` in the same directory ([about_Profiles](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_profiles?view=powershell-7.5)).
- Windows PowerShell 5.1: directory `$HOME\Documents\WindowsPowerShell\` (visible in the same page's remote-session example: `"$HOME\Documents\WindowsPowerShell\Microsoft.PowerShell_profile.ps1"`); ISE uses `Microsoft.PowerShellISE_profile.ps1` `[verify]`, not fetched.
- The Documents folder can be redirected or OneDrive-managed, so `$PROFILE` must be queried, never assembled (about_Profiles note).
- The two editions use different files, so a `grim shell init` setup must be written twice on Windows.

**Execution policy.**
- Windows client default when no scope sets one is `Restricted`; `RemoteSigned` is the default on Windows Server, and non-Windows is `Unrestricted` and not enforced ([about_Execution_Policies](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_execution_policies?view=powershell-7.5)). Restricted blocks profiles. The 5.1 page says the same; I read the 7.5 page only, so the 5.1 default is `[verify]`.
- The policy governs script files and profiles; the page says "users can easily bypass a policy by typing the script contents at the command line", and the Restricted entry says "Permits individual commands". An evaluated string (`Invoke-Expression`) is therefore not a script file and is not blocked, but the profile line that contains it is itself in a `.ps1` file that the policy must allow. Practical result: if the policy blocks the profile, nothing in grim's init ever runs, and there is nothing grim can do about it.

**Constrained Language Mode** ([about_Language_Modes](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_language_modes?view=powershell-7.5)). Applies automatically under AppLocker or WDAC. In it, `Add-Type` cannot compile arbitrary C#, `New-Object` and type literals are limited to an allow-list, and .NET method calls are restricted to allowed types. `[System.IO.File]` and `[System.Diagnostics.Process]` are not on the allow-list shown on that page, so file-time checks and `Process.Start` fail under CLM; `Get-Item`, `Test-Path` and cmdlets keep working. A design that must survive CLM needs a cmdlet fallback or a "do nothing" fallback inside `try { } catch { }`.

## 3. Cost

All numbers below are anecdotal or from a single machine unless stated.

- **Idle native spawn on Windows (mise's own measurement):** the code comment in mise's pwsh template says a no-op hook-env spawn "costs a whole process (~22ms, almost entirely CreateProcess) to learn nothing" ([pwsh.rs](https://raw.githubusercontent.com/jdx/mise/main/src/shell/pwsh.rs)). The same project's change [PR 7073](https://github.com/jdx/mise/pull/7073) (merged 2025-11-26) reports `hook-env` going "~40-60ms → ~8-10ms" on the idle path (platform not stated in the excerpt I read `[verify]`).
- **Windows versus Linux process creation:** one microbenchmark finds Linux ">20x faster" than Windows at create-and-reap and notes it "is very sensitive to ... Windows Defender" ([bitsnbites](https://www.bitsnbites.eu/benchmarking-os-primitives/), date not checked, likely OLD `[verify]`); an older report cites ~100 ms per process with Kaspersky and ~1.7 ms per CreateProcess in a Git profile ([Stack Overflow mirror](https://kaweah.freedombox.rocks/kiwix/stackoverflow.com_en_all_2023-11/questions/10710912/what-is-the-process-creation-overhead-in-windows), OLD). No controlled Defender-on versus off measurement on current Windows found.
- **Init script evaluation at profile load:** a profile-timing write-up reports `oh-my-posh init pwsh | Invoke-Expression` at ~0.38-0.48 s (dominating the profile) and a starship init at ~110 ms in a gist comment ([gist](https://gist.github.com/diogotito/199448685b8d4bf1064d644628fb0feb), [mattkotsenas](https://mattkotsenas.azurewebsites.net/posts/pwsh-profiling-async-startup); neither fetched in full, edition not stated `[verify]`). These are spawn-plus-eval costs, not the cost of evaluating a small static script. I found no per-edition number for evaluating a ~100-line script; measure it (`Measure-Command` with and without) before committing a budget.
- **Per-prompt `Test-Path` versus .NET calls:** one Reddit loop test reports `Test-Path` 846 ms versus `[FileInfo].Exists` 149 ms over the same (several thousand) files, 2020, PowerShell version not stated ([thread mirror](https://red.127.pp.ua/r/PowerShell/comments/jcx4je/testpath_vs_systemiofileinfo_exists), OLD, anecdotal). The direction (cmdlet overhead dominates) is consistent; absolute per-call numbers for a prompt with 2-3 checks were not found. Microsoft's own startup guide lists profile processing as a main startup phase ([startup-performance](https://learn.microsoft.com/en-us/powershell/scripting/dev-cross-plat/performance/startup-performance)) but gives no timings.
- **Background-job creation:** Microsoft's own example shows `Start-Job` ~4.8 s for five jobs versus `Start-ThreadJob` ~0.6 s ([Start-ThreadJob](https://learn.microsoft.com/en-us/powershell/module/threadjob/start-threadjob?view=powershell-7.5)); "results may vary". `Start-Job` starts a new PowerShell process per job, which is why it is slow; that mechanism is not stated on the page I fetched `[verify]`.

## 4. File-time comparison without spawning

Cheapest in both editions: .NET static methods, no cmdlet binding.

```powershell
# a newer than b; a missing -> 1601-01-01, so "never newer"; b missing -> a is newer
[System.IO.File]::GetLastWriteTimeUtc($a) -gt [System.IO.File]::GetLastWriteTimeUtc($b)

# a older than N seconds (missing a => 1601 => always "older": treat as stale)
([DateTime]::UtcNow - [System.IO.File]::GetLastWriteTimeUtc($a)).TotalSeconds -gt $n
```

- Missing-file behaviour: documented "returns 12:00 midnight, January 1, 1601 A.D." and does not throw ([File.GetLastWriteTimeUtc](https://learn.microsoft.com/en-us/dotnet/api/system.io.file.getlastwritetimeutc?view=net-9.0)). A missing watch file thus compares as ancient, and a missing state file compares as "older than everything", so a missing state file would trip the guard on every prompt unless it is created. Decide the semantics explicitly.
- `Get-Item`/`Test-Path` add cmdlet and provider cost (anecdotal figures above) and throw on a missing path unless `-ErrorAction` is set; `[System.IO.File]::Exists` returns false for directories.
- Strings: `$PWD.Path` is a string compare and needs no call. `Get-Location` is a cmdlet; for non-FileSystem providers `$PWD.ProviderPath` can differ (starship and zoxide both normalise via `Get-Location`/`ProviderPath` and skip non-FileSystem providers, see their sources above).
- Under CLM, `[System.IO.File]` is not on the allow-list (see section 2), so wrap in `try/catch` and fall back to `(Get-Item -LiteralPath $a -ErrorAction Ignore).LastWriteTimeUtc`.

## 5. Detached background process

Facts established from sources:

- **`powershell.exe -WindowStyle Hidden` flashes a console.** Root cause in the maintainers' words: "The console window is automatically created by the OS when the process starts"; the hidden option is processed after the window exists. In 2017 the maintainer said "there's no code change we can do in powershell.exe to address this"; the issue is still open, with an in-flight PR 27111 and a milestone of "Future" ([PowerShell issue 3028](https://github.com/PowerShell/PowerShell/issues/3028)). This concerns launching **PowerShell itself**; launching a native `grim.exe` from `Start-Process -WindowStyle Hidden` is a different case and I did not find a source that settles whether it flashes `[verify]`.
- **Win32 flags** ([Process Creation Flags](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags), 2025-07): `DETACHED_PROCESS` (0x8): "the new process does not inherit its parent's console"; `CREATE_NO_WINDOW` (0x08000000): console app runs "without a console window"; "ignored ... if it is used with either CREATE_NEW_CONSOLE or DETACHED_PROCESS"; `CREATE_NEW_PROCESS_GROUP` (0x200) disables Ctrl+C for the group; `CREATE_BREAKAWAY_FROM_JOB` only works if the job sets `JOB_OBJECT_LIMIT_BREAKAWAY_OK`.
- **.NET `Process.Start` / `ProcessStartInfo.CreateNoWindow`:** used by starship for its synchronous call; starship does not detach. The .NET runtime calls `CreateProcess` with handle inheritance on, so a long-lived child can hold a captured pipe open; a .NET 11 post says "By default, `InheritedHandles` is `null`, so all inheritable handles are still inherited" and adds `StartDetached` (uses `DETACHED_PROCESS` on Windows, `setsid` on Unix) ([.NET 11 Process API](https://devblogs.microsoft.com/dotnet/process-api-improvements-in-dotnet-11/)). .NET 11 is not what PowerShell 5.1 or current 7.x embeds, so neither API is available to an init script today `[verify]`.
- **Pipe inheritance hazard:** a pipe reader does not see EOF until every write handle closes, so a detached child that inherited the stdout of a captured call keeps the capture waiting ([Stack Overflow mirror](https://kaweah.freedombox.rocks/kiwix/content/stackoverflow.com_en_all_2023-11/questions/53231519/freeze-when-programmatically-launching-a-batch-file-with-output-redirection-wh), OLD). In the prompt this bites only if the prompt is itself invoked under a capture; it bites hard in `Invoke-Expression (& grim shell init | Out-String)` if init ever spawns the worker. Spawn only from the prompt, never during init.
- **`Start-ThreadJob`:** runs "in separate threads within the local process" ([doc](https://learn.microsoft.com/en-us/powershell/module/threadjob/start-threadjob?view=powershell-7.5)); it therefore cannot survive the shell's exit, and a native process started from it is just a child of the same process. Module availability in 5.1 requires installing `ThreadJob` from the Gallery `[verify]`.
- **`&` background operator and `Start-Job`:** 7 only (about_Operators page above); "functionally equivalent to `Start-Job`".
- **Antivirus delay:** no source found quantifying Defender delay for spawning a small native binary; the bitsnbites and Stack Overflow sources above show it dominates process creation when present (anecdotal, OLD).

How update-notifier style tools do it:

- Node `update-notifier` spawns a detached child and drops the handles: `spawn(process.execPath, [check.js, JSON.stringify(options)], { detached: true, stdio: 'ignore' }).unref()` ([source](https://raw.githubusercontent.com/sindresorhus/update-notifier/main/update-notifier.js)). The result is written to a config file and read by the next run. This is exactly requirement (4), but done by the tool, not by a shell script. Node's `detached` maps to `DETACHED_PROCESS` on Windows `[verify]`.
- Rust equivalent: `std::os::windows::process::CommandExt::creation_flags` ([docs](https://doc.rust-lang.org/std/os/windows/process/trait.CommandExt.html)) with `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` and null stdio, which needs no PowerShell-side logic at all.
- None of the surveyed shell tools (zoxide, mise, direnv, starship, oh-my-posh, fnm, atuin) starts a detached background process from their PowerShell init; the `OnIdle` event in oh-my-posh is for a streamed prompt, not a background task.

Evidence for and against each technique:

| Technique | For | Against |
|---|---|---|
| `Start-Process -WindowStyle Hidden` | one line, works in 5.1 and 7 | flash for launching powershell.exe is documented (issue 3028); effect for a native console exe unverified `[verify]`; `Start-Process` itself is a cmdlet call on every trigger (cost not measured) |
| `[Process]::Start` + `CreateNoWindow` | no cmdlet; starship proves it works in both editions | `CREATE_NO_WINDOW` gives the child a hidden console of its own and inherits handles; not `DETACHED_PROCESS` semantics; CLM blocks it |
| `Start-Job` | built in | a new PowerShell process per job (~1 s per job in the Microsoft example), dies with the session |
| `Start-ThreadJob` | cheap (~0.1 s per job in the example) | in-process, dies with the session; 5.1 needs a module install |
| `&` (7 only) | short | equals `Start-Job`; 7 only |
| `grim` spawns its own detached worker (Rust, `DETACHED_PROCESS`) | removes all edition differences; same code path on every shell; proven pattern (update-notifier) | one extra short native spawn per interval from the prompt; code in grim, not in the script |

## 6. Completions

What `clap_complete`'s PowerShell generator emits (source: [powershell.rs](https://raw.githubusercontent.com/clap-rs/clap/master/clap_complete/src/aot/shells/powershell.rs)):

```powershell
using namespace System.Management.Automation
using namespace System.Management.Automation.Language

Register-ArgumentCompleter -Native -CommandName '{bin_name}' -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)

    $commandElements = $commandAst.CommandElements
    $command = @(
        '{bin_name}'
        for ($i = 1; $i -lt $commandElements.Count; $i++) {
    ...
    $completions = @(switch ($command) { ... [CompletionResult]::new('-s', '-s', [CompletionResultType]::ParameterName, 'help') ... })
    $completions.Where{ $_.CompletionText -like "$wordToComplete*" } | Sort-Object -Property ListItemText
}
```

- **Static data, no process spawn at completion time** (the whole command tree is a `switch`). Cost at load is parsing one script block plus the registration; I found no published timing. Cost grows with the number of subcommands and flags (grim has 26 subcommands); measure.
- **Editions:** the template uses `.Where{}` and `[CompletionResult]::new(...)`, both present in 5.1 and 7; I found no report of it failing in 5.1 `[verify]`.
- **Evaluated inside an init script:** the two `using namespace` lines must be the first statements of the parsed text. PowerShell's rule is that a `using` statement must appear before any other statement `[verify]` against the language spec (not fetched). Embedded after other lines, or piped line-by-line to `Invoke-Expression`, it fails; the `clap_complete` dynamic-completion fix (merged 2024-12-17, [PR 5848](https://github.com/clap-rs/clap/pull/5848)) added `Out-String` and a here-string for related reasons. Remedy for grim: either emit the `using` lines at the very top of the init script and evaluate it with `Out-String`, or strip them and fully qualify the three types.
- **`grim.exe` versus `grim`:** the generator registers one `-CommandName` equal to the bin name given to clap. Whether PowerShell matches a completer registered for `grim` when the user types `grim.exe` (or the reverse) is not documented in anything I fetched `[verify]`. Cheapest safe option is to generate for both names (the bin name is a parameter to `clap_complete::generate`) and register two completers; test with Pester.
- Alternative the survey found: mise ships a separate pre-generated `completions/mise.ps1` file in its tree; the dynamic completion environment path in `clap_complete::env` exists but spawns the binary per completion, which conflicts with "static".

## 7. Non-interactive and agent contexts

- Profile loading is controlled by `-NoProfile`; `-NonInteractive` only prevents prompts for input ([about_Pwsh](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_pwsh?view=powershell-7.5); the 5.1 flags are in [about_PowerShell_exe](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_powershell_exe?view=powershell-5.1)). The pages I fetched do not state in one sentence that `-Command` and `-File` load the profile by default; widely known, not quoted `[verify]`. Because of this, mise's tests start child shells with `-NoProfile` "so a developer profile cannot pre-activate" ([e2e-win/activate_strictmode.Tests.ps1](https://raw.githubusercontent.com/jdx/mise/main/e2e-win/activate_strictmode.Tests.ps1)).
- Profiles are not run automatically in remote sessions, and `$PROFILE` is unpopulated there (about_Profiles above).
- Whether `prompt` ever runs for `pwsh -Command ...`: the prompt function is called only when the host reads an interactive line; a one-shot command does not display a prompt. I did not find a source that states this directly `[verify]`. The practical consequence for grim is that in non-interactive runs the **init script executes but the prompt hook never fires**, so init-time work (completion registration, wrapper definition) must be cheap and must not spawn anything.
- Detecting interactivity inside the init script: I found no source documenting `[Environment]::UserInteractive`, `$Host.Name -eq 'ConsoleHost'` or `[Console]::IsOutputRedirected` for this purpose `[verify]`. What the surveyed tools actually do: atuin hard-requires PSReadLine (`if (!(Get-Module PSReadLine -ErrorAction Ignore)) { Write-Error ... }`), starship calls `Set-PSReadLineOption` unguarded and mise wraps its PSReadLine-dependent part in `try { } catch {}` / "PSReadLine is absent in ..." comment. A `$Host.Name`/`Get-Module PSReadLine` check is therefore the observed pattern, not a documented one.
- AI-agent shells: a published issue shows an agent's `pwsh -Command` tool failing on Windows when pwsh was installed via winget, unrelated to profiles ([claudeissues.com 55796](https://claudeissues.com/issue/55796-powershell-tool-fails-with-exit-code-1-on-windows-when-pwsh-installed-via-winget), not fetched `[verify]`). No evidence on whether agent harnesses pass `-NoProfile`. Assume either behaviour and make init idempotent and silent when not interactive.

## 8. Printing a notice from inside `prompt`

- A prompt must return an object; if the function writes to the host and returns nothing, PowerShell falls back to the default `PS>` prompt ([about_Prompts](https://learn.microsoft.com/powershell/module/microsoft.powershell.core/about/about_prompts)).
- Output in a prompt function goes to the success stream and becomes part of the prompt string; `Write-Host` goes straight to the host. mise's wrapper only calls the previous prompt and lets its output pass through (shown above); zoxide discards its own hook output with `$null = __zoxide_hook`.
- PSReadLine caveat: PSReadLine infers prompt text for its syntax-error colouring, and an "impure" prompt (one that calls `Write-Host`) can defeat that inference ([PSReadLine issue summary](https://lightrun.com/answers/powershell-psreadline-syntax-error-prompt-does-not-work-when-prompt-function-is-defined-in-profile-or-launch-command-and-u), mirror, not fetched in full `[verify]`). starship sidesteps redraw problems by calling `Set-PSReadLineOption -ExtraPromptLineCount` with the number of extra lines in its own returned text.
- Printing a notice **before** calling the wrapped prompt adds lines above the prompt rather than to it, so the extra-line count of the prompt is unchanged. This is an inference, not a documented behaviour `[verify]` — test it under PSReadLine with a multi-line prompt (starship) in the same profile.
- ANSI: Windows PowerShell 5.1's engine "doesn't output ANSI escape sequences" itself, but "Windows Console Host is xterm compatible" on Windows 10+, so a literal `[char]27 + '[...m'` can be written ([about_ANSI_Terminals 5.1](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_ansi_terminals?view=powershell-5.1)). `` `e `` is not valid in 5.1; use `[char]27`. I did not find a source that confirms VT is enabled by default in conhost on every 5.1 host `[verify]`. Safest: use `Write-Host -ForegroundColor` for colour in 5.1 and skip ANSI.

## 9. Testing

| Tool | How PowerShell integration is tested | Evidence |
|---|---|---|
| mise | Pester suite in `e2e-win/` run on a Windows runner via `pwsh e2e-win\run.ps1`; each case that needs a clean shell spawns a child with `-NoProfile`; test names map to bug classes (strict mode, error hygiene, double dash, pipeline input, POSIX PATH) | [test-impl.yml](https://raw.githubusercontent.com/jdx/mise/main/.github/workflows/test-impl.yml) (lines mention `pwsh e2e-win\run.ps1`), [e2e-win tree](https://github.com/jdx/mise/tree/main/e2e-win) |
| direnv | `test/direnv-test.ps1` shell script invoked by its Go test, plus `shell_pwsh_test.go`; no Pester, and none of its three workflows (codeql, go, release) runs the `.ps1` test in the file list I fetched | [test/direnv-test.ps1](https://raw.githubusercontent.com/direnv/direnv/master/test/direnv-test.ps1), [workflows](https://github.com/direnv/direnv/tree/master/.github/workflows) |
| zoxide | `completions.rs` plus the Rust shell tests; no PowerShell-specific CI file found; `ci.yml` branches on `windows-latest` | [ci.yml](https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/.github/workflows/ci.yml) |
| starship, oh-my-posh, fnm, atuin | I found no Pester or per-shell integration test in my tree listings (starship tree search matched nothing; atuin has only the script and its Rust shim); oh-my-posh tree search matched no Pester file. Absence from a GitHub tree-search is weak evidence `[verify]` | trees fetched via the GitHub API |

Breakage classes documented by mise's own tests and comments (each links to an issue or discussion where given):

- **Strict mode** reading unset globals ([discussion 5312](https://github.com/jdx/mise/discussions/5312)).
- **`$Error` pollution** from the deactivation preamble (`activate_error_hygiene.Tests.ps1`).
- **Double-dash `--`** swallowed by the parameter binder: "PowerShell's parameter binder claims the first bare `--` on a line as its own end-of-parameters token and drops it before $args is populated" ([discussion 13196](https://github.com/jdx/mise/discussions/13196)); this affects any `grim` wrapper function that forwards `@args`, so a native-command alias is safer than a function wrapper.
- **Stdin pipeline** reaching the wrapped command (`activate_pipeline.Tests.ps1`).
- **In-process activation leaking state across tests**, hence child processes for every case.
- **Prompt ordering/double-wrap** is not covered by those tests; no upstream issue was found.

Recommended CI shape (inference from the above): GitHub Actions `windows-latest` with `shell: pwsh` and `shell: powershell` jobs running a Pester file; each test that activates does so in a child process with `-NoProfile`; extra Linux/macOS `pwsh` job. The repo-local Windows runner described in the owner's global notes can reproduce this on a Windows host.

## 10. Windows `cmd.exe`

There is no native hook equivalent to a prompt function in `cmd.exe`; the credible route is Clink. oh-my-posh documents: "There's no out-of-the-box support for Windows CMD when it comes to custom prompts", and recommends Clink, which "since Clink v1.7.0 has builtin support for Oh My Posh" (`clink config prompt use oh-my-posh`) ([docs source](https://raw.githubusercontent.com/JanDeDobbeleer/oh-my-posh/main/website/docs/installation/prompt.mdx)). zoxide lists a third-party `clink-zoxide` integration rather than shipping one ([README](https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/README.md)). I found no evidence that mise, direnv, fnm or atuin support `cmd.exe` `[verify]`, and starship's Clink support I could not confirm from the fetched docs `[verify]`. Verdict: treat `cmd.exe` as out of scope, document Clink as a user-managed route.

## Recommendation

This section is opinion, separate from the findings above.

1. **Hook:** wrap the global `prompt` function in the zoxide/mise shape: sentinel-guarded, saves `$function:prompt`, calls it last, preserves `$LASTEXITCODE`, replays `$?`. Write it in 5.1-safe grammar. Do not depend on `LocationChangedAction` (7-only; and the prompt always runs after it anyway). Do not use `OnIdle` (oh-my-posh's own comment records a host-crash class under rapid prompt cycles). Document "init grim after starship/oh-my-posh" because they do not chain.
2. **Idle check:** one comparison per prompt using `[System.IO.File]::GetLastWriteTimeUtc` and `$PWD.Path` against cached script-scope variables; no `Test-Path`, no `Get-ChildItem`, no spawn; `try/catch` around it for CLM. Make the missing-file semantics explicit (a missing state file must not trip the guard every prompt).
3. **Guard-trip and notice:** call `grim` as a native command (one fast run), print one line with `Write-Host` (colour via `-ForegroundColor`, no ANSI) **before** returning the prompt, and test it against starship's multi-line prompt.
4. **Background refresh:** have `grim` itself spawn the detached worker (Rust `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`, null stdio) and return immediately, as `update-notifier` does; the script only invokes `grim` once per interval after a timestamp check. This removes the 5.1/7 differences, the console-flash and the pipe-inheritance risks from PowerShell. Never spawn during init.
5. **Completions:** embed `clap_complete`'s output with the `using namespace` lines at the very top of the evaluated text (or stripped and fully qualified); consume with `Invoke-Expression (& grim shell init powershell | Out-String)`; register for `grim` and `grim.exe` unless a test shows one is enough.
6. **Encoding:** keep the init output ASCII; if a notice can contain non-ASCII, set `[Console]::OutputEncoding` to UTF-8 around the call and restore it, as zoxide does.
7. **Tests:** Pester on `windows-latest` under both `powershell` and `pwsh`, child process with `-NoProfile` per case, plus cases for strict mode, clean `$Error`, `--` forwarding, piped stdin and an idle prompt that spawns nothing (assert process count).
8. **Measure before committing numbers:** time init evaluation and the idle prompt (100 prompts) in 5.1 and 7 on a Windows host with Defender on; the sources give only anecdotes.

## Sources

| URL | Type | Date | Covers |
|---|---|---|---|
| https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/templates/powershell.txt | source code | main, 2026-11 | zoxide hook, UTF-8 wrapper, StrictMode guard |
| https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/README.md | docs | main | init line, "end of config", clink-zoxide |
| https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/.github/workflows/ci.yml | CI config | main | windows-latest matrix |
| https://raw.githubusercontent.com/jdx/mise/main/src/shell/pwsh.rs | source code | main | prompt wrap, chpwd, 22 ms comment |
| https://github.com/jdx/mise/pull/7073 | PR | merged 2025-11-26 | hook-env fast path 40-60 -> 8-10 ms |
| https://raw.githubusercontent.com/jdx/mise/main/.github/workflows/test-impl.yml | CI config | main | `pwsh e2e-win\run.ps1` |
| https://raw.githubusercontent.com/jdx/mise/main/e2e-win/activate_strictmode.Tests.ps1 | test | main | strict-mode class, -NoProfile child |
| https://raw.githubusercontent.com/jdx/mise/main/e2e-win/activate_error_hygiene.Tests.ps1 | test | main | $Error hygiene |
| https://raw.githubusercontent.com/jdx/mise/main/e2e-win/activate_double_dash.Tests.ps1 | test | main | `--` binder issue |
| https://raw.githubusercontent.com/jdx/mise/main/e2e-win/activate_pipeline.Tests.ps1 | test | main | stdin pipeline |
| https://github.com/jdx/mise/discussions/5312 | discussion | referenced in test comments, not fetched | strict mode bug |
| https://github.com/jdx/mise/discussions/13196 | discussion | referenced in test comments, not fetched | `--` bug |
| https://raw.githubusercontent.com/direnv/direnv/master/internal/cmd/shell_pwsh.go | source code | master | LocationChangedAction, 7.2 gate |
| https://raw.githubusercontent.com/direnv/direnv/master/test/direnv-test.ps1 | test | master | pwsh test script |
| https://raw.githubusercontent.com/starship/starship/master/src/init/starship.ps1 | source code | master | prompt replace, ProcessStartInfo, 5.1 branching |
| https://raw.githubusercontent.com/JanDeDobbeleer/oh-my-posh/main/src/shell/scripts/omp.ps1 | source code | main | prompt replace, OnIdle |
| https://raw.githubusercontent.com/JanDeDobbeleer/oh-my-posh/main/website/docs/installation/prompt.mdx | docs | main | cmd.exe via Clink |
| https://raw.githubusercontent.com/Schniz/fnm/master/src/shell/powershell.rs | source code | master | cd alias hook |
| https://raw.githubusercontent.com/atuinsh/atuin/main/crates/atuin/src/shell/atuin.ps1 | source code | main | PSReadLine handlers |
| https://raw.githubusercontent.com/clap-rs/clap/master/clap_complete/src/aot/shells/powershell.rs | source code | master | clap_complete PowerShell output |
| https://github.com/clap-rs/clap/pull/5848 | PR | merged 2024-12-17 | Out-String, here-string for evaluated completions (fetch summary; exact wording unverified) |
| https://raw.githubusercontent.com/sindresorhus/update-notifier/main/update-notifier.js | source code | main | detached spawn + unref |
| https://github.com/PowerShell/PowerShell/issues/3028 | issue | opened 2017 (OLD, but active 2026-03) | -WindowStyle Hidden console flash |
| https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags | MS docs | 2025-07-14 | DETACHED_PROCESS, CREATE_NO_WINDOW, breakaway |
| https://devblogs.microsoft.com/dotnet/process-api-improvements-in-dotnet-11/ | MS blog | 2026-05 | InheritedHandles, StartDetached |
| https://doc.rust-lang.org/std/os/windows/process/trait.CommandExt.html | Rust docs | current | `creation_flags` |
| https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_profiles?view=powershell-7.5 | MS docs | 2026-05-01 | profile paths, remote sessions |
| https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_execution_policies?view=powershell-7.5 | MS docs | 2026-08-31 | execution policy defaults, not a security boundary |
| https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_language_modes?view=powershell-7.5 | MS docs | 2026-01-26 | CLM restrictions |
| https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_pwsh?view=powershell-7.5 | MS docs | 2026-01-18 | -NoProfile, -NonInteractive |
| https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_powershell_exe?view=powershell-5.1 | MS docs | 5.1 | 5.1 CLI flags |
| https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_operators?view=powershell-7.5 | MS docs | 2026-03-24 | `&&`, `??`, ternary, background `&` |
| https://learn.microsoft.com/en-us/powershell/scripting/whats-new/what-s-new-in-powershell-70?view=powershell-7.5 | MS docs | current | ternary and chain operators in 7.0 |
| https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_automatic_variables?view=powershell-5.1 | MS docs | 5.1 | no `$IsWindows` in 5.1 |
| https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_automatic_variables?view=powershell-7.5 | MS docs | 7.5 | `$IsWindows` |
| https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_character_encoding?view=powershell-7.5 | MS docs | current | `$OutputEncoding` |
| https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_ansi_terminals?view=powershell-5.1 | MS docs | 5.1 | ANSI in 5.1 (search snippet, not fully fetched) |
| https://learn.microsoft.com/powershell/module/microsoft.powershell.core/about/about_prompts | MS docs | current | prompt must return object (search snippet) |
| https://learn.microsoft.com/en-us/powershell/module/threadjob/start-threadjob?view=powershell-7.5 | MS docs | 2026-01-18 | in-process thread jobs, job cost example |
| https://learn.microsoft.com/en-us/powershell/scripting/dev-cross-plat/performance/startup-performance | MS docs | 2026-01-14 | startup phases |
| https://learn.microsoft.com/en-us/dotnet/api/system.io.file.getlastwritetimeutc?view=net-9.0 | MS docs | current | missing file returns 1601-01-01 |
| https://github.com/PowerShell/PowerShell/issues/7233 | issue | not fetched (OLD) | console encoding behaviour |
| https://gist.github.com/mklement0/ef57aea441ea8bd43387a7d7edfc6c19 | gist | not fetched (likely OLD) | native output encoding |
| https://www.powershellgallery.com/packages/ScrewCitySoftware.PwshProfile/0.7.3/Content/Public%5CTools%5CEnable-Zoxide.ps1 | third-party script | not dated | oh-my-posh/zoxide ordering failure mode |
| https://gist.github.com/diogotito/199448685b8d4bf1064d644628fb0feb | gist | not dated | init timing anecdotes |
| https://mattkotsenas.azurewebsites.net/posts/pwsh-profiling-async-startup | blog | not dated | init timing anecdotes |
| https://red.127.pp.ua/r/PowerShell/comments/jcx4je/testpath_vs_systemiofileinfo_exists | forum mirror | 2020 (OLD) | Test-Path vs .NET timing, anecdotal |
| https://www.bitsnbites.eu/benchmarking-os-primitives/ | blog | not dated, likely OLD | Windows vs Linux process creation |
| https://kaweah.freedombox.rocks/kiwix/stackoverflow.com_en_all_2023-11/questions/10710912/what-is-the-process-creation-overhead-in-windows | Q&A mirror | OLD | CreateProcess and antivirus |
| https://kaweah.freedombox.rocks/kiwix/content/stackoverflow.com_en_all_2023-11/questions/53231519/freeze-when-programmatically-launching-a-batch-file-with-output-redirection-wh | Q&A mirror | OLD | pipe inheritance hang |
| https://lightrun.com/answers/powershell-psreadline-syntax-error-prompt-does-not-work-when-prompt-function-is-defined-in-profile-or-launch-command-and-u | issue mirror | not dated | impure prompt vs PSReadLine inference |
| https://claudeissues.com/issue/55796-powershell-tool-fails-with-exit-code-1-on-windows-when-pwsh-installed-via-winget | issue | not fetched | agent pwsh tool failure |
