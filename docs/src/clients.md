# Client Compatibility

grim installs one canonical artifact into many AI clients, and not every
client can host every artifact kind. A skill is universal, but a rule needs a
per-file scoping surface that some clients lack, and an agent needs a shipped
file format that fewer still provide.

Writing a rule into a client that silently drops its path scoping — or an agent
into one that never reads it — is worse than an honest refusal: the config
looks installed but does nothing. grim renders only what each client can
faithfully host, degrades with a warning where a surface exists but loses
fidelity, and declines (warn, skip, zero files) where no ownable surface exists
at all.

This page is the enforced source of truth. A table-parity test in
`src/install/client_target.rs` reads this matrix when the test suite runs and
fails if any cell drifts from the `Vendor` implementations, so the
documentation cannot silently lie about what is supported.

Legend:

- `✓` — supported: a native surface, or a faithful transform.
- `◐` — supported with a documented limitation (see [Known gaps](#known-gaps)).
- `✗` — declined: no ownable surface, so grim warns, skips, and writes nothing
  (see [Known gaps](#known-gaps)).

## Support matrix {#matrix}

<!--
  Client marks reused from docs/theme/index.hbs: inlined from
  @lobehub/icons-static-svg (MIT, Copyright (c) 2025 LobeHub) for every
  client except Zed and Warp, whose marks come from Simple Icons
  (CC0-1.0). Droid carries no mark — neither set has one. The `agents`
  folder glyph is drawn, not licensed: that row is a directory, not a
  vendor.
-->

<div class="matrix-table">

| Client | Skill | Rule | Agent | MCP |
|--------|-------|------|-------|-----|
| <svg viewBox="0.00 0.50 24.00 24.00" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M20.998 10.949H24v3.102h-3v3.028h-1.487V20H18v-2.921h-1.487V20H15v-2.921H9V20H7.488v-2.921H6V20H4.487v-2.921H3V14.05H0V10.95h3V5h17.998v5.949zM6 10.949h1.488V8.102H6v2.847zm10.51 0H18V8.102h-1.49v2.847z"></path></svg> [Claude] | ✓ | ✓ | ✓ | ✓ |
| <svg viewBox="1.78 1.78 20.44 20.44" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M16 6H8v12h8V6zm4 16H4V2h16v20z"></path></svg> [OpenCode] | ✓ | ◐ | ✓ | ◐ |
| <svg viewBox="-0.20 -0.20 24.40 24.40" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M19.245 5.364c1.322 1.36 1.877 3.216 2.11 5.817.622 0 1.2.135 1.592.654l.73.964c.21.278.323.61.323.955v2.62c0 .339-.173.669-.453.868C20.239 19.602 16.157 21.5 12 21.5c-4.6 0-9.205-2.583-11.547-4.258-.28-.2-.452-.53-.453-.868v-2.62c0-.345.113-.679.321-.956l.73-.963c.392-.517.974-.654 1.593-.654l.029-.297c.25-2.446.81-4.213 2.082-5.52 2.461-2.54 5.71-2.851 7.146-2.864h.198c1.436.013 4.685.323 7.146 2.864zm-7.244 4.328c-.284 0-.613.016-.962.05-.123.447-.305.85-.57 1.108-1.05 1.023-2.316 1.18-2.994 1.18-.638 0-1.306-.13-1.851-.464-.516.165-1.012.403-1.044.996a65.882 65.882 0 00-.063 2.884l-.002.48c-.002.563-.005 1.126-.013 1.69.002.326.204.63.51.765 2.482 1.102 4.83 1.657 6.99 1.657 2.156 0 4.504-.555 6.985-1.657a.854.854 0 00.51-.766c.03-1.682.006-3.372-.076-5.053-.031-.596-.528-.83-1.046-.996-.546.333-1.212.464-1.85.464-.677 0-1.942-.157-2.993-1.18-.266-.258-.447-.661-.57-1.108-.32-.032-.64-.049-.96-.05zm-2.525 4.013c.539 0 .976.426.976.95v1.753c0 .525-.437.95-.976.95a.964.964 0 01-.976-.95v-1.752c0-.525.437-.951.976-.951zm5 0c.539 0 .976.426.976.95v1.753c0 .525-.437.95-.976.95a.964.964 0 01-.976-.95v-1.752c0-.525.437-.951.976-.951zM7.635 5.087c-1.05.102-1.935.438-2.385.906-.975 1.037-.765 3.668-.21 4.224.405.394 1.17.657 1.995.657h.09c.649-.013 1.785-.176 2.73-1.11.435-.41.705-1.433.675-2.47-.03-.834-.27-1.52-.63-1.813-.39-.336-1.275-.482-2.265-.394zm6.465.394c-.36.292-.6.98-.63 1.813-.03 1.037.24 2.06.675 2.47.968.957 2.136 1.104 2.776 1.11h.044c.825 0 1.59-.263 1.995-.657.555-.556.765-3.187-.21-4.224-.45-.468-1.335-.804-2.385-.906-.99-.088-1.875.058-2.265.394zM12 7.615c-.24 0-.525.015-.84.044.03.16.045.336.06.526l-.001.159a2.94 2.94 0 01-.014.25c.225-.022.425-.027.612-.028h.366c.187 0 .387.006.612.028-.015-.146-.015-.277-.015-.409.015-.19.03-.365.06-.526a9.29 9.29 0 00-.84-.044z"></path></svg> [Copilot] | ✓ | ✓ | ✓ | ◐ |
| <svg viewBox="-1.71 -1.72 27.43 27.43" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M8.086.457a6.105 6.105 0 013.046-.415c1.333.153 2.521.72 3.564 1.7a.117.117 0 00.107.029c1.408-.346 2.762-.224 4.061.366l.063.03.154.076c1.357.703 2.33 1.77 2.918 3.198.278.679.418 1.388.421 2.126a5.655 5.655 0 01-.18 1.631.167.167 0 00.04.155 5.982 5.982 0 011.578 2.891c.385 1.901-.01 3.615-1.183 5.14l-.182.22a6.063 6.063 0 01-2.934 1.851.162.162 0 00-.108.102c-.255.736-.511 1.364-.987 1.992-1.199 1.582-2.962 2.462-4.948 2.451-1.583-.008-2.986-.587-4.21-1.736a.145.145 0 00-.14-.032c-.518.167-1.04.191-1.604.185a5.924 5.924 0 01-2.595-.622 6.058 6.058 0 01-2.146-1.781c-.203-.269-.404-.522-.551-.821a7.74 7.74 0 01-.495-1.283 6.11 6.11 0 01-.017-3.064.166.166 0 00.008-.074.115.115 0 00-.037-.064 5.958 5.958 0 01-1.38-2.202 5.196 5.196 0 01-.333-1.589 6.915 6.915 0 01.188-2.132c.45-1.484 1.309-2.648 2.577-3.493.282-.188.55-.334.802-.438.286-.12.573-.22.861-.304a.129.129 0 00.087-.087A6.016 6.016 0 015.635 2.31C6.315 1.464 7.132.846 8.086.457zm-.804 7.85a.848.848 0 00-1.473.842l1.694 2.965-1.688 2.848a.849.849 0 001.46.864l1.94-3.272a.849.849 0 00.007-.854l-1.94-3.393zm5.446 6.24a.849.849 0 000 1.695h4.848a.849.849 0 000-1.696h-4.848z"></path></svg> [Codex] | ✓ | ✗ | ✓ | ◐ |
| <svg viewBox="-0.84 -0.84 25.69 25.69" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M22.106 5.68L12.5.135a.998.998 0 00-.998 0L1.893 5.68a.84.84 0 00-.419.726v11.186c0 .3.16.577.42.727l9.607 5.547a.999.999 0 00.998 0l9.608-5.547a.84.84 0 00.42-.727V6.407a.84.84 0 00-.42-.726zm-.603 1.176L12.228 22.92c-.063.108-.228.064-.228-.061V12.34a.59.59 0 00-.295-.51l-9.11-5.26c-.107-.062-.063-.228.062-.228h18.55c.264 0 .428.286.296.514z"></path></svg> [Cursor] | ✓ | ✓ | ✓ | ◐ |
| <svg viewBox="-0.57 -0.44 24.87 24.87" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M4.594 6.677C6.67-2.226 18.746-2.211 21.16 6.632c.353 1.297 1.725 7.582-1.673 13.747-1.545 2.797-5.841 5.49-6.99 1.883C8.6 25.477 3.315 24.1 5.789 18.609l-.318.143c-3.57 1.305-3.863-1.208-3.173-2.513.45-.84.727-1.335.937-1.897.353-.975.458-1.568.593-2.498.27-1.837.277-3.607.765-5.167zm8.37.01a.92.92 0 00-.81.428c-.217.323-.33.825-.33 1.462 0 .705.15 1.89 1.14 1.89h.008c.757 0 1.214-.705 1.214-1.89 0-.622-.127-1.125-.367-1.455a1.014 1.014 0 00-.855-.435zm4.08 0a.92.92 0 00-.81.428c-.217.323-.33.825-.33 1.462 0 .705.15 1.89 1.14 1.89h.008c.757 0 1.215-.705 1.215-1.89 0-.622-.128-1.125-.368-1.455a1.014 1.014 0 00-.855-.435z"></path></svg> [Kiro] | ✓ | ✓ | ✗ | ◐ |
| <svg viewBox="-1.71 -1.71 27.43 27.43" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M24 9.333C24 18.666 20 24 9.333 24H8v-8h1.333C14 16 16 14 16 9.333V8h8v1.333zM8 16H0V8h8v8zM16 8H8V0h8v8z"></path></svg> [Junie] | ✓ | ◐ | ✗ | ◐ |
| <svg viewBox="-0.57 -0.57 25.14 25.14" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M20.616 10.835a14.147 14.147 0 01-4.45-3.001 14.111 14.111 0 01-3.678-6.452.503.503 0 00-.975 0 14.134 14.134 0 01-3.679 6.452 14.155 14.155 0 01-4.45 3.001c-.65.28-1.318.505-2.002.678a.502.502 0 000 .975c.684.172 1.35.397 2.002.677a14.147 14.147 0 014.45 3.001 14.112 14.112 0 013.679 6.453.502.502 0 00.975 0c.172-.685.397-1.351.677-2.003a14.145 14.145 0 013.001-4.45 14.113 14.113 0 016.453-3.678.503.503 0 000-.975 13.245 13.245 0 01-2.003-.678z"></path></svg> [Gemini] | ✓ | ✗ | ✓ | ◐ |
| <svg viewBox="-1.71 -1.71 27.43 27.43" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M2.25 1.5a.75.75 0 0 0-.75.75v16.5H0V2.25A2.25 2.25 0 0 1 2.25 0h20.095c1.002 0 1.504 1.212.795 1.92L10.764 14.298h3.486V12.75h1.5v1.922a1.125 1.125 0 0 1-1.125 1.125H9.264l-2.578 2.578h11.689V9h1.5v9.375a1.5 1.5 0 0 1-1.5 1.5H5.185L2.562 22.5H21.75a.75.75 0 0 0 .75-.75V5.25H24v16.5A2.25 2.25 0 0 1 21.75 24H1.655C.653 24 .151 22.788.86 22.08L13.19 9.75H9.75v1.5h-1.5V9.375A1.125 1.125 0 0 1 9.375 8.25h5.314l2.625-2.625H5.625V15h-1.5V5.625a1.5 1.5 0 0 1 1.5-1.5h13.19L21.438 1.5z"/></svg> [Zed] | ✓ | ✗ | ✗ | ◐ |
| <svg viewBox="-1.82 -1.71 27.43 27.43" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M15.087 23.18L12.03 24l-2.097-7.823-5.738 5.738-2.251-2.251 5.718-5.719-7.769-2.082.82-3.057 11.294 3.08 3.08 11.295z"></path><path d="M19.505 18.762l-3.057.82-2.564-9.573-9.572-2.564.819-3.057 11.295 3.079 3.08 11.295z"></path><path d="M23.893 14.374l-3.057.82-2.565-9.572L8.7 3.057 9.52 0l11.295 3.08 3.079 11.294z"></path></svg> [Amp] | ✓ | ✗ | ✗ | ◐ |
| <svg viewBox="2.30 2.30 19.39 19.39" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M3 6.5A2.5 2.5 0 0 1 5.5 4h3.4a2 2 0 0 1 1.5.7l1.1 1.3h7A2.5 2.5 0 0 1 21 8.5v9a2.5 2.5 0 0 1-2.5 2.5h-13A2.5 2.5 0 0 1 3 17.5v-11Zm5 6.2a1.3 1.3 0 1 0 0 2.6 1.3 1.3 0 0 0 0-2.6Zm4 0a1.3 1.3 0 1 0 0 2.6 1.3 1.3 0 0 0 0-2.6Zm4 0a1.3 1.3 0 1 0 0 2.6 1.3 1.3 0 0 0 0-2.6Z"></path></svg> [agents] | ✓ | ✗ | ✗ | ✗ |
| <svg viewBox="-1.16 -1.11 26.33 26.33" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M21.751 22.607c1.34 1.005 3.35.335 1.508-1.508C17.73 15.74 18.904 1 12.037 1 5.17 1 6.342 15.74.815 21.1c-2.01 2.009.167 2.511 1.507 1.506 5.192-3.517 4.857-9.714 9.715-9.714 4.857 0 4.522 6.197 9.714 9.715z"></path></svg> [Antigravity] | ✓ | ✗ | ✓ | ◐ |
| <svg viewBox="-1.41 -1.45 26.89 26.89" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M17.035 3.991c2.75 0 4.98 2.24 4.98 5.003v1.667l1.45 2.896a1.01 1.01 0 01-.002.909l-1.448 2.864v1.668c0 2.762-2.23 5.002-4.98 5.002H7.074c-2.751 0-4.98-2.24-4.98-5.002V17.33l-1.48-2.855a1.01 1.01 0 01-.003-.927l1.482-2.887V8.994c0-2.763 2.23-5.003 4.98-5.003h9.962zM8.265 9.6a2.274 2.274 0 00-2.274 2.274v4.042a2.274 2.274 0 004.547 0v-4.042A2.274 2.274 0 008.265 9.6zm7.326 0a2.274 2.274 0 00-2.274 2.274v4.042a2.274 2.274 0 104.548 0v-4.042A2.274 2.274 0 0015.59 9.6z"></path><path d="M12.054 5.558a2.779 2.779 0 100-5.558 2.779 2.779 0 000 5.558z"></path></svg> [Cline] | ✓ | ✗ | ✗ | ✗ |
| <span class="mark" aria-hidden="true">D</span> [Droid] | ✓ | ✗ | ✗ | ✗ |
| <svg viewBox="-1.71 -1.71 27.43 27.43" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M21.595 23.61c1.167-.254 2.405-.944 2.405-.944l-2.167-1.784a12.124 12.124 0 01-2.695-3.131 12.127 12.127 0 00-3.97-4.049l-.794-.462a1.115 1.115 0 01-.488-.815.844.844 0 01.154-.575c.413-.582 2.548-3.115 2.94-3.44.503-.416 1.065-.762 1.586-1.159.074-.056.148-.112.221-.17.003-.002.007-.004.009-.007.167-.131.325-.272.45-.438.453-.524.563-.988.59-1.193-.061-.197-.244-.639-.753-1.148.319.02.705.272 1.056.569.235-.376.481-.773.727-1.171.165-.266-.08-.465-.086-.471h-.001V3.22c-.007-.007-.206-.25-.471-.086-.567.35-1.134.702-1.639 1.021 0 0-.597-.012-1.305.599a2.464 2.464 0 00-.438.45l-.007.009c-.058.072-.114.147-.17.221-.397.521-.743 1.083-1.16 1.587-.323.391-2.857 2.526-3.44 2.94a.842.842 0 01-.574.153 1.115 1.115 0 01-.815-.488l-.462-.794a12.123 12.123 0 00-4.049-3.97 12.133 12.133 0 01-3.13-2.695L1.332 0S.643 1.238.39 2.405c.352.428 1.27 1.49 2.34 2.302C1.58 4.167.73 3.75.06 3.4c-.103.765-.063 1.92.043 2.816.726.317 1.961.806 3.219 1.066-1.006.236-2.11.278-2.961.262.15.554.358 1.119.64 1.688.119.263.25.52.39.77.452.125 2.222.383 3.164.171l-2.51.897a27.776 27.776 0 002.544 2.726c2.031-1.092 2.494-1.241 4.018-2.238-2.467 2.008-3.108 2.828-3.8 3.67l-.483.678c-.25.351-.469.725-.65 1.117-.61 1.31-1.47 4.1-1.47 4.1-.154.486.202.842.674.674 0 0 2.79-.861 4.1-1.47.392-.182.766-.4 1.118-.65l.677-.483c.227-.187.453-.37.701-.586 0 0 1.705 2.02 3.458 3.349l.896-2.511c-.211.942.046 2.712.17 3.163.252.142.509.272.772.392.569.28 1.134.49 1.688.64-.016-.853.026-1.956.261-2.962.26 1.258.75 2.493 1.067 3.219.895.106 2.051.146 2.816.043a73.87 73.87 0 01-1.308-2.67c.811 1.07 1.874 1.988 2.302 2.34h-.001z"></path></svg> [Goose] | ✓ | ✗ | ✗ | ✗ |
| <svg viewBox="-0.06 -0.06 24.12 24.12" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M12.035 2.723h9.253A2.712 2.712 0 0 1 24 5.435v10.529a2.712 2.712 0 0 1-2.712 2.713H8.047Zm-1.681 2.6L6.766 19.677h5.598l-.399 1.6H2.712A2.712 2.712 0 0 1 0 18.565V8.036a2.712 2.712 0 0 1 2.712-2.712Z"></path></svg> [Warp] | ✓ | ✗ | ✗ | ✗ |
| <svg viewBox="-1.13 -0.45 26.25 26.25" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M9.046 7.104a.527.527 0 110 1.055.527.527 0 010-1.055z"></path><path d="M15.376 7.104a.528.528 0 110 1.056.528.528 0 010-1.056z"></path><path clip-rule="evenodd" d="M16.877 1.912c.58-.27 1.14-.323 1.616-.037a.317.317 0 01-.326.542c-.227-.136-.547-.153-1.022.068-.352.165-.765.45-1.234.866 2.683 1.17 4.4 3.5 5.148 5.921a6.421 6.421 0 00-.704.184c-.578.016-1.174.204-1.502.735-.338.55-.268 1.276.072 2.069l.005.012.007.014c.523 1.045 1.318 1.91 2.2 2.284-.912 3.274-3.44 6.144-5.972 6.988v2.109h-2.11v-2.11c-1.043.417-2.086.01-2.11 0v2.11h-2.11v-2.11c-2.531-.843-5.061-3.713-5.973-6.987.882-.373 1.678-1.238 2.2-2.284l.007-.014.006-.012c.34-.793.41-1.518.071-2.069-.327-.531-.923-.719-1.503-.735a6.409 6.409 0 00-.704-.183c.749-2.421 2.466-4.751 5.149-5.922-.47-.416-.88-.701-1.234-.866-.474-.221-.794-.204-1.021-.068a.318.318 0 01-.435-.109.317.317 0 01.109-.433c.476-.286 1.036-.233 1.615.037.49.229 1.031.628 1.621 1.182A9.924 9.924 0 0112 2.568c1.199 0 2.284.19 3.256.526.59-.554 1.13-.953 1.62-1.182zM8.835 6.577a1.266 1.266 0 100 2.532 1.266 1.266 0 000-2.532zm6.33 0a1.267 1.267 0 100 2.533 1.267 1.267 0 000-2.533z"></path><path d="M.395 13.118c-.966-1.932-.163-3.863 2.41-3.365v-.001l.05.01c.084.018.17.038.26.06.033.009.067.017.1.027.084.022.168.048.255.076l.09.027c.528 0 .95.158 1.16.501.212.343.212.87-.105 1.61-.085.17-.178.333-.276.489l-.01.017a4.967 4.967 0 01-.62.791l-.019.02c-1.092 1.117-2.496 1.336-3.295-.262z"></path><path d="M21.193 9.753c2.574-.5 3.378 1.433 2.411 3.365-.58 1.159-1.476 1.361-2.342.96l-.011-.005a2.419 2.419 0 01-.114-.056l-.019-.01a2.751 2.751 0 01-.115-.067l-.023-.014c-.035-.022-.071-.044-.106-.068l-.05-.035c-.55-.388-1.062-1.007-1.44-1.76-.276-.647-.311-1.132-.174-1.472.176-.439.636-.639 1.23-.639.032-.011.066-.02.099-.03.08-.026.16-.05.238-.072l.117-.03a5.502 5.502 0 01.3-.067z"></path></svg> [OpenClaw] | ✓ | ✗ | ✗ | ✗ |
| <svg viewBox="-1.71 -1.71 27.43 27.43" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M0 0v24h24V0H0zm22.222 22.222H1.778V1.778h20.444v20.444zm-7.555-4.964h2.222v1.778h-2.794L12.89 17.83v-2.794h1.778v2.222zm4 0h-1.778v-2.222h-2.222v-1.778h2.793l1.207 1.207v2.793zm-7.556-2.591H9.333v-1.778h1.778v1.778zm-5.778-1.778h1.778v4h4v1.778H6.54L5.333 17.46V12.89zm13.334-3.556v1.778h-5.778V9.333h1.987V7.111h-1.987V5.333h2.558l1.206 1.207v2.793h2.014zm-11.556-2h2.222l1.778 1.778v2H9.333v-2H7.111v2H5.333V5.333h1.778v2zm4 0H9.333v-2h1.778v2z"></path></svg> [Kilo] | ✓ | ✗ | ✗ | ✗ |

</div>

Bundles decompose into their member kinds and are not a column.

`agents` is not a product — it is the vendor-neutral target. Selecting it
installs one copy of each skill into the cross-vendor `.agents/skills` pool
rather than into any one client's directory. That pool is read by more clients
than write to it — see [Shared skills pool visibility](#gap-shared-pool) for
who scans it. Rules, agents, and MCP have no vendor-neutral format, so
`agents` declines all three. It is never detected, only selected: request it
explicitly with `--client agents` or in `[options].clients`.

It is also what grim falls back to when *nothing* is detected: rather than
writing a directory for every client it knows about, an install with no
`--client`, no `[options].clients`, and no client marker present targets
`agents` alone. If the declared set holds nothing it can install, the command
exits `78` and names both ways to select a client.

## What grim will and will not do about a gap {#compensation}

The matrix above says what each client can host. This section says what grim
does when a client cannot host something — because "the client cannot do this"
is not, by itself, an answer to whether grim should make it work anyway.

**grim renders artifacts. grim does not run inside your client.** Three
things follow, and they hold for every client on this page:

- **grim always repairs its own output.** If grim wrote a file, grim owns how
  that file behaves. Installing a path-scoped rule and getting unconditional
  context instead is grim's bug, not a client limitation, and it gets fixed —
  including by registering an entry in a client's own settings file where that
  is what the client reads.
- **grim may render around a gap.** Where a client cannot express something an
  artifact declares, grim can sometimes say it a different way — writing the
  lost scope into the rule text, or generating an extra file the client already
  knows how to read. They are ordinary generated files: deterministic,
  removed when you uninstall, and inert if grim is never run again.
- **grim will never install a plugin, extension, or any other runtime code
  into your client.** Several gaps on this page could be closed that way. None
  will be. Code loaded into a client breaks on that client's schedule, not
  grim's, and it breaks *quietly* — your rules stop loading, nothing errors,
  and the agent just gets worse. A declined cell that you can see beats a shim
  that silently stops working.

Where a gap is not closed, the [Known gaps](#known-gaps) entry below says so
and points at the upstream issue. Support is only ever added, never withdrawn
(see [Stability and Versioning](./stability.md)), so a decline today is a
decline that can become support later without breaking anything you have
installed.

## Known gaps {#known-gaps}

Every ◐ and ✗ above traces to a specific, verified upstream limitation. The
internal working list is the vendor capability watchlist; the entries below are
its user-facing projection — the rationale and the upstream tracking pointer
for each, plus a couple of authoring caveats (like [Cursor]'s comma-in-glob
split) worth calling out even where the surface is otherwise fully supported.

### MCP: ws and oauth are Claude-only {#gap-mcp-ws-oauth}

Every MCP cell except [Claude] is ◐ because grim declines two descriptor shapes
for every client other than [Claude]: the WebSocket (`ws`) transport and the
structured `oauth` block. No surveyed client other than [Claude] documents a
native config surface for either, so grim skips a ws- or oauth-bearing server
for that client with a warning rather than writing an entry the client cannot
honor. Every other transport (stdio, sse, http) registers normally.

[Antigravity] is the one close call. Its MCP docs name websocket alongside sse
and streamable HTTP as taking the same `serverUrl` field, which read literally
would make it a second `ws` target — but that rests on a single sentence grim
could not confirm against raw upstream page text, and adding support later is
additive while withdrawing it would be a breaking change. grim therefore
declines `ws` there too, and revisits it on confirmation.

### Copilot: global MCP environment references {#gap-copilot-env}

At global scope, the [GitHub Copilot][copilot] CLI does not substitute `${VAR}`
environment references in its MCP config, so grim skips a descriptor that
carries one (project scope is unaffected). Upstream shipped substitution in
v0.0.406 and regressed it in v0.0.407 — grim will drop the skip once a fixed
release is confirmed.

### Cursor: a comma inside a glob splits the pattern {#gap-cursor-globs}

[Cursor] rules are fully supported (a `.mdc` file with a comma-joined `globs`
string), but Cursor splits that string on **every** comma — including a comma
inside a `{a,b}` brace alternation ([cursor forum #76648][cursor-glob-split]).
A single glob such as `src/**/*.{rs,toml}` is therefore read as two separate
patterns. grim writes the glob unchanged and emits a warning at install time so
you can split the rule into one pattern per glob.

### OpenCode: rules install without path scoping {#gap-opencode-rules}

[OpenCode] has a per-file rules surface but no `paths:` scoping. A rule installs
as body-plus-provenance with its `paths` dropped and a warning — Degraded, not
declined, because the instruction content still installs and loads.

### Codex: rules declined {#gap-codex-rules}

[Codex] has no path-scoped instruction mechanism — its `AGENTS.md` is always-on
and directory-granular, with no `paths`/`applyTo` equivalent. grim declines a
rule for [Codex]: warn, skip, and write no file.

### Kiro: global rules are inert until #9176 {#gap-kiro-rules}

[Kiro] steering rules are native at both scopes, but a global-scope scoped rule
is written correctly yet ignored by [Kiro] until upstream bug [kiro #9176] is
fixed. grim writes the correct `fileMatch` steering and emits a warning citing
the issue; the file self-heals (becomes active) when the bug closes, with no
grim change.

A manual workaround exists today: switching the steering block to
`inclusion: auto` makes [Kiro] load it heuristically at the global scope. grim
deliberately does not emit `auto` — it ships the deterministic, path-scoped
`fileMatch` the rule actually describes, which activates exactly where intended
once the upstream fix lands, instead of a fuzzy always-on heuristic.

### Kiro: agents declined {#gap-kiro-agents}

A native [Kiro] IDE agent format exists, but the [Kiro] CLI expects an
incompatible JSON schema in the same `.kiro/agents/` directory (open bug
[kiro #8040]). Writing IDE-format files could break CLI users, so grim declines
[Kiro] agents pending a resolution.

### Junie: rules lose scoping and are project-only, agents declined {#gap-junie}

[Junie] rules install into `.junie/rules/<name>.md` at **project scope** with
their `paths` dropped and a warning. The blocker is **scoping, not
ownability** — an earlier version of this page said [Junie] had "no
grim-ownable per-file rules surface", and that was wrong. `.junie/rules/` is a
real per-file directory grim can own; it sits above the legacy guidelines file
in [Junie]'s own discovery order. What it lacks is any per-file activation
key: every Markdown file in the directory is concatenated automatically, so a
rule's `paths` has nowhere to land and the rule loads unconditionally.

At **global scope** grim writes nothing at all. There is no `~/.junie/rules/`
upstream, so a global rule is skipped with a warning and records zero outputs
rather than landing in a directory [Junie] never reads.

[Junie]'s `.junie/agents/` format exists but is early-access-preview only, not
generally available; agents are declined until it ships.

### Gemini: rules declined, agents gated by a setting {#gap-gemini}

[Gemini]'s only rules surface is the `GEMINI.md` hierarchy, with no ownable
per-file target, so rules are declined. [Gemini] agents are native and are
installed, but [Gemini] only loads them when `experimental.enableAgents` is set —
which defaults on, so they work out of the box for most users.

The individual-tier [Gemini] CLI (free/Pro/Ultra) stopped being served on
2026-06-18, [transitioning to the Antigravity CLI][gemini-antigravity] (which
reportedly carries Agent Skills and subagents forward — unverified). Enterprise
[Gemini] Code Assist licenses remain fully supported; grim's [Gemini] support
targets that surface, verified against the still-served enterprise docs.

### Shared skills pool visibility {#gap-shared-pool}

**Reading the pool and writing to it are different lists**, and conflating
them is the usual source of confusion here.

*Writing* — grim installs skills into `.agents/skills` by default for
[Codex], [Gemini], [Zed], [Amp], [Goose], the vendor-neutral `agents` target,
and — at **project scope only** — [Antigravity]. A skill installed for any one
of them is physically the same file, so it is discoverable by every client
that scans the directory even when only one was selected. [Goose] is a full
member at both scopes; [Antigravity] is partial, because its *global* skills
live under its own `~/.gemini/config/skills`, so a global install for it is a
separate copy.

*Reading* — several more clients scan the pool without grim writing there by
default. [Cursor], [Copilot], [OpenCode] and [Warp] all read it at both
scopes, but each has a first-class directory of its own upstream, and grim
prefers a vendor-specific location wherever one exists. [Kilo] reads the
**project** pool but has no global support, and [OpenClaw] reads the
**global** pool but has no project scope; both are partial readers, and grim
installs both to their own directories.

Set `[options.vendors.<name>].shared_skills` on any full pool reader —
[Cursor], [Copilot], [OpenCode] or [Warp] — to move that client's skills into
the pool instead. The key is refused (exit `65` at `grim config set`, `78`
when hand-authored) for a client that is not a verified pool reader, because
enabling it there would write where nothing reads. Partial readers are
excluded on purpose: membership is scope-blind, so a client that reads the
pool at only one scope would get skills written where it never scans at the
other.

Which clients *read* the pool is upstream scan behavior, not a grim choice: a
skill installed for one member is visible to every other member whether or
not you asked for that. Which clients grim *writes* it for is partly a
choice — yours. [Goose] and project-scope [Antigravity] land there because
their own vendors point there, but every opt-in above is you deciding to
trade a client's native layout for that wider visibility. Left unset, the
client keeps its own directory.

grim refcounts the shared directory so removing one client never deletes a
skill another client still records — subject to one [documented
boundary](./stability.md#limitations-pool-refcount): that refcount reads
install state, not the filesystem.

### Zed: rules and agents declined, MCP env references {#gap-zed}

[Zed] has no rule scoping — instruction files follow a nine-name first-match
precedence with no per-file ownership — so rules are declined. [Zed] agents run
over ACP with no installable file format and are declined too. [Zed]'s MCP config
has no environment-reference substitution, so grim skips a `${VAR}`-bearing
server with a warning.

### Amp: rules and agents declined {#gap-amp}

[Amp]'s only instruction surface is `AGENTS.md` (falling back to `AGENT.md`, then
`CLAUDE.md`) with no per-file scoping, so rules are declined. [Amp] subagents are
spawned at runtime with no installable file format, so agents are declined.

### Antigravity: rules declined, project detection is opt-in {#gap-antigravity}

[Antigravity] documents a workspace `.agents/rules` folder, but grim declines
rules for it on two counts. Global rules are a single `~/.gemini/GEMINI.md` —
not a per-file surface, and a file [Gemini] writes to as well
([gemini-cli #16058][antigravity-rules-collision]), so grim cannot own it. And
no rule-file frontmatter key was found for the workspace folder: scoping is
described as a glob-based "activation mode" configured in the product, so a
rule written there would silently lose its `paths`. grim declines rather than
install a rule that looks scoped and is not.

[Antigravity] is also never auto-detected in a workspace. All of its
project-scope surfaces live under `.agents/`, which [Codex], [Gemini], [Zed],
[Amp], [Goose] and `agents` also use — detecting on it would install
[Antigravity] files
into every workspace that has ever used any pool client. Upstream documents no
product-specific project marker, so grim reports none: request it explicitly
with `--client antigravity` or in `[options].clients`. Global scope is detected
normally, from `~/.gemini/config`.

One consequence of that root worth knowing: `~/.gemini/config` sits *inside*
`~/.gemini`, which is [Gemini]'s own global marker. A global install for
[Antigravity] therefore creates a directory that makes [Gemini] detected too,
so a later autodetected global command will target both. Pass `--client` (or
set `[options].clients`) if you want only one of them.

Uninstalling does not undo it. grim removes the files it installed but leaves
the now-empty directories, so `~/.gemini` survives and [Gemini] stays detected.
Remove the empty tree by hand if you want that signal gone. The reverse never
happens: a global [Gemini] install creates `~/.gemini/agents`, never
`~/.gemini/config`, so [Gemini] alone never makes [Antigravity] detected.

This client targets the **Antigravity 2.0** desktop product. The Antigravity
CLI (`agy`) and the Antigravity IDE read *different* global skill directories
(`~/.gemini/antigravity-cli/skills/` and `~/.gemini/antigravity/skills/`), so
their users are not served by this client name; both can be added later under
their own names.

### The skills-only clients {#gap-skills-only}

[Cline], [Droid], [Goose], [Warp], [OpenClaw] and [Kilo] install **skills
only**. Rules, agents and MCP are declined for all six, but not for the same
reason in every case. Four of them — [Droid], [Goose], [Warp] and [OpenClaw] —
document no per-file rules surface that can express a rule's `paths`. For
[Cline] and [Kilo] the decline is a **scheduling** decision rather than a
capability one: this release ships skills for these clients, and rule support
is additive to add later. None of the six ships an installable subagent file
format, and grim writes no MCP config for any of them. Each decline is additive
to reverse — support can be added later without breaking anything, while
withdrawing it could not be.

Client-specific detail worth knowing:

- **[Cline]** is the one whose rules decline is *not* about a missing
  capability. Its `.clinerules/` genuinely documents per-file `paths:`
  scoping — the exact mechanism whose absence forces a decline elsewhere. It
  is declined here only because this release ships skills for these clients;
  it is the strongest candidate to gain rule support next. Cline is also a
  documented **non-adopter** of the shared `.agents/skills` pool: its own docs
  list `.cline/skills/`, `.clinerules/skills/` and `.claude/skills/`, and the
  pool appears nowhere.
- **[Droid]** is Factory's agent. The client is named `droid` but its directory
  is `.factory/` — grim names the client, not the vendor org, the same way it
  uses `claude` rather than `anthropic`. Factory also documents a compatibility
  directory `.agent/skills/`, singular, which is a different convention from
  the `.agents` pool; grim writes neither.
- **[Goose]** is the one client here that installs into the shared
  `.agents/skills` pool rather than its own directory, because Goose's own docs
  label `.goose/skills/` backward-compatibility and name `.agents/skills` the
  recommended location. Its skills are therefore visible to every other pool
  client, as described under [Shared skills pool visibility](#gap-shared-pool).
- **[Warp]** reads the pool too, but its own `.warp/skills/` is a first-class
  location upstream, so grim installs there by default. Set
  `[options.vendors.warp].shared_skills` to move them into the pool instead.
  `~/.warp/` is the same path on macOS, Linux and Windows.
- **[OpenClaw]** installs at **global scope only**. It has no per-repository
  concept: the path its documentation calls "project" is a fixed daemon home
  under `~/.openclaw/workspace` that does not follow the repository you run
  grim in, so installing there would mix unrelated projects' skills into one
  directory. A project-scope install warns, writes nothing, and records no
  outputs. Use `--global`.
- **[Kilo]** was formerly Kilo Code, and grim uses the current name. It writes
  `.kilo/` exclusively; the older `.kilocode/` is deprecated upstream and grim
  never writes it, though an existing one is still recognized when detecting
  whether Kilo is in use.

## The `compatibility:` frontmatter field {#compatibility-disclaimer}

An artifact may carry a free-text `compatibility:` frontmatter field. It is an
editor and runtime *hint* only — a note for humans and tools that read the
source. It has **zero effect** on how grim renders or gates an artifact per
client. A `compatibility: codex` line does not make a rule install for [Codex],
and it never overrides the matrix above. This matrix — enforced by the
parity test — is the authoritative statement of what grim installs
where.

<!-- external -->
[claude]: https://code.claude.com
[opencode]: https://opencode.ai
[copilot]: https://github.com/features/copilot
[codex]: https://developers.openai.com/codex
[cursor]: https://cursor.com
[kiro]: https://kiro.dev
[junie]: https://www.jetbrains.com/junie/
[gemini]: https://geminicli.com
[zed]: https://zed.dev
[amp]: https://ampcode.com
[antigravity]: https://antigravity.google
[cline]: https://cline.bot
[droid]: https://factory.ai
[goose]: https://block.github.io/goose
[warp]: https://warp.dev
[openclaw]: https://github.com/openclaw/openclaw
[kilo]: https://kilo.ai
[antigravity-rules-collision]: https://github.com/google-gemini/gemini-cli/issues/16058
[cursor-glob-split]: https://forum.cursor.com/t/76648
[kiro #9176]: https://github.com/kirodotdev/Kiro/issues/9176
[kiro #8040]: https://github.com/kirodotdev/Kiro/issues/8040
[gemini-antigravity]: https://developers.googleblog.com/an-important-update-transitioning-gemini-cli-to-antigravity-cli/

<!-- internal -->
[agents]: #gap-shared-pool
