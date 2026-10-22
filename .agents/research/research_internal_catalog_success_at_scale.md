# Research: Internal catalog success at 10k+ entries

## Metadata

**Date:** 2026-10-17
**Domain:** packaging
**Triggered by:** discussion `.agents/discussions/index-scale-rollout.md`
**Expires:** 2027-04-17

Scope: how organisations running catalogs of 10k+ reusable artifacts define and measure success, what goes wrong, and how often such catalogs change. Evidence only; no recommendation. Source quality is flagged per item (V = vendor/secondary, P = primary doc/paper, F = forum/blog). Several figures are third-party or single-source and are marked.

## 1. Reference points for scale

| Catalog | Size | Source |
|---|---|---|
| Spotify internal Backstage | 14,000+ software components, 1,600+ engineers, 120+ plugins by 60 teams (2021); "tens of thousands of components", 800 teams (current marketing page) | [Spotify Eng 2021](https://engineering.atspotify.com/2021/03/happy-birthday-backstage-spotifys-biggest-open-source-project-grows-up-fast/) P; [backstage.spotify.com](https://backstage.spotify.com/build-like-spotify) V |
| Backstage at Roadie customers | "multiple tens of thousands of entities"; challenges start at "1,000s and 10,000s" | [Roadie](https://roadie.io/blog/scaling-backstage-intro) V |
| Artifact Hub | 21,764 packages, 503,732 snapshots (API `/api/v1/stats`, fetched 2026-10-17) | [Artifact Hub API](https://artifacthub.io/api/v1/stats) P |
| Official MCP Registry | 36,176 servers at 2026-09-26 (third-party count; no official figure found) | [developertoolkit.ai](https://developertoolkit.ai/en/ecosystem/mcp/registry-and-gateways/) V |
| skills.sh | 2,179 (2026-01-16) to 40,285 (2026-02-05) listed skills, 18.5x in 20 days; 19,830 with >=20 installs across 2,419 repos | [arXiv 2602.08004](https://arxiv.org/html/2602.08004v1) P; [arXiv 2607.00911](https://arxiv.org/html/2607.00911v1) P |
| Public SKILL.md on GitHub | 138,133 files in 20,556 repos | [arXiv 2608.08453](https://arxiv.org/abs/2608.08453) P |
| Anthropic internal skills | "hundreds of them in active use"; no exact count | [Anthropic blog](https://claude.dev/blog/lessons-from-building-claude-code-how-we-use-skills/) P |
| Homebrew core | ~8,313 formulae, 1.3 GB tap clone (undated third-party) | [search result, brew.sh 4.0.0 context](https://brew.sh/2023/02/16/homebrew-4.0.0) |

No published case of an enterprise running 10k+ internal AI-config artifacts (skills/rules/agents) was found. The closest analogues are Backstage (component catalogs), public skill registries, and MCP registries.

## 2. How success is defined and measured

**Backstage / IDP vendors (completeness, correctness, ownership)**
- Completeness = coverage of real assets, usually scoped ("every business-critical service" or "a subset of teams"), rarely absolute. Roadie: absolute completeness "is rare". [Roadie](https://roadie.io/blog/measuring-catalog-correctness-and-completeness/) V
- Correctness = required fields present and valid (name, description, slug annotation regex, `type` from an enum, tier/region labels), aggregated as a pass/fail scorecard per entity. Same source. The page proposes no numeric targets and says nothing on staleness.
- Datadog Catalog Health report measures completeness by entity type, lifecycle, tier, definition version and ownership, and lists entities with no owner. [Datadog docs](https://docs.datadoghq.com/internal_developer_portal/eng_reports/catalog_health/) V
- Ownership coverage = owned assets / total assets, as a percentage. [Atlan docs](https://docs.atlan.com/platform/lakehouse/references/metadata-completeness-tracking) V (a data-catalog product, not a software catalog).
- Dexcom case: completeness "past 95%" after org-wide rollout, then a scaffolder that requires owner, type and lifecycle at repo creation to prevent regression. [Roadie case study](https://roadie.io/case-studies/dexcom-automating-catalog-completeness-backstage/) V (vendor self-report).
- Adoption: Spotify cites a 2x productivity figure from internal DevEx metrics (no method given) and the claim of ~99% voluntary internal adoption is repeated second-hand (no primary source found). Outside Spotify, "average adoption 10%" is attributed to Spotify officials by TechTarget; the denominator (developers vs teams) is not defined. [TechTarget](https://www.techtarget.com/searchITOperations/news/366558592/Behind-the-scenes-Spotify-Backstage-a-work-in-progress) V; forum echoes at [tianpan.co](https://tianpan.co/forum/t/backstage-holds-89-market-share-but-only-10-internal-adoption-why/417) F.

**AI-config specifics**
- Anthropic measures skill use with a `PreToolUse` hook that logs skill invocations, to find skills "popular or undertriggering compared to our expectations". No central approval team; skills start in a GitHub sandbox folder, and the owner opens a PR to the marketplace "once a skill has gotten traction (which is up to the skill owner to decide)". No numeric threshold. [Anthropic blog](https://claude.dev/blog/lessons-from-building-claude-code-how-we-use-skills/) P
- Same post: checked-in skills each add to model context; a marketplace lets "your team decide which ones to install" as you scale. It does not address duplicates or retirement.
- Skill-quality proxies in research: share of skills with valid routing metadata (retrievability from startup descriptions), clone/duplicate rate, update recency, downstream pass-rate delta (below).

**Search / findability metrics**
- Definitions exist (zero-results rate = unique zero-result searches / unique searches; search success rate = sessions with a click; time to success = median seconds to first result click). [Adobe docs](https://experienceleague.adobe.com/en/docs/commerce/live-search/live-search-admin/performance), [Prefixbox](https://www.prefixbox.com/hu/case-studies/diy-search-suite) V.
- Benchmarks found are ecommerce only (zero-result average 10-15%, target <5% per [helloretail](https://helloretail.com/nl/blog/2026-02-24-ecommerce-search-statistics/) V). No intranet or developer-catalog benchmark found.
- InnerSource: an industrial case study ranked discoverability, communication channels and ownership of reusable assets as top improvement priorities. [EASE 2023](https://www.se.cs.uni-saarland.de/conferences/ASE/ase2023/details/ease-2023/ease-2023-easier/3/Using-InnerSource-for-Improving-Internal-Reuse-An-Industrial-Case-Study.html) P. InnerSource Commons lists non-owner commits, organisational distance and repo counts as metrics. [ISC GQM](https://innersourcecommons.gitbook.io/managing-innersource-projects/measuring/index/use_gqm/index-2) P.

**Usage and long tail**
- No source reports "fraction of catalog entries ever used" for an internal catalog. Public data:
  - skills.sh: mean installs by category range 42 (Local File Control) to 1,268 (Web Search); adoption "concentrates on a few general skills" but no distribution is quantified. [arXiv 2602.08004](https://arxiv.org/html/2602.08004v1) P
  - Artifactory: no measured never-downloaded share; JFrog notes many artifacts "will never be used" (CI snapshots) and gives download-age cleanup rules. [JFrog](https://jfrog.com/help/r/artifactory-cleanup-best-practices/artifactory-cleanup-best-practices) V
  - npm: 61% of packages had no release in the prior 12 months (2019 analysis, release activity, not downloads). Search results only; not re-verified.
- Popularity is not shown to predict utility: in a downstream study (37,596 deduplicated skills; 87 tasks), benchmark skills lifted pass rate 4.6 to 19.5 points, but on 36.78% of tasks the same skills helped in some model/harness configurations and hurt in others; top-ranked candidate was beaten by a lower-ranked one in 23.19% of groups. The paper reports no download-vs-utility correlation. [arXiv 2610.08875](https://arxiv.org/html/2610.08875) P

## 3. What goes wrong at scale

**Duplication and near-duplication (strongest evidence, AI-config specific)**
- 46.3% of skills.sh listings share a normalised name with another listing; some names appear >100 times. Authors say name matching undercuts true duplicates; embedding match judged unreliable. [arXiv 2602.08004](https://arxiv.org/html/2602.08004v1) P
- A separate 42,447-skill crawl found most skills unique, with duplicates mainly from cross-platform listing: the duplicate rate depends on whether cross-listings count. [arXiv 2601.10338](https://arxiv.org/html/2601.10338v1) P (as summarised by search; not re-fetched)
- Clone detection on 137,470 skills: 1.06M clone pairs involving 66.8% of skills; 95.3% of pairs cross author boundaries; in name-based families 97% of pairs are modified variants and 3% byte-identical; 67% of skills in name families are superseded by a higher-quality variant. A later version reports 258K relationships and 40% cross-author (versions disagree; verify which before citing). [arXiv 2603.22447](https://arxiv.org/abs/2603.22447) P
- Reuse is "largely a one-time copy": 70.3% of linked registry-to-repo pairs are >=0.99 similar; of reused-but-unmodified skills, 40.2% had upstream change that never reached the copy; of reused skills edited locally while upstream also changed, 62.3% diverged independently. 76% of personal-use skills are unlinked to any registry skill. [arXiv 2607.00911](https://arxiv.org/html/2607.00911v1) P
- Supply is concentrated: top 10 authors produce 24.7% of skills (Gini 0.746); one archive repo held 8,187 skills. [arXiv 2602.08004 summary](https://arxiv.org/html/2602.08004v1), [arXiv 2607.00911](https://arxiv.org/html/2607.00911v1) P

**Quality defects**
- 91.8% of 138,133 public SKILL.md files have at least one detected defect (88.8-94.6% across thresholds); dominant: weak routing metadata, bloated or non-actionable bodies, poor resource organisation. Valid routing metadata improved retrieval in a 20,000-skill stress test. AI-marked skills had more safety/portability problems. [arXiv 2608.08453](https://arxiv.org/abs/2608.08453) P
- Median skill 1,414 tokens, max 116,239; many long skills bundle multiple components. 9% rated critical-risk and 30% elevated by LLM audit (labels unvalidated by humans). [arXiv 2602.08004](https://arxiv.org/html/2602.08004v1) P

**Catalog rot, orphans, ownership**
- Hand-maintained `catalog-info.yaml` is "hard to keep current across an organisation" (Roadie); advice is to ingest from authoritative sources (IdP, GitHub, k8s) and keep manual YAML for small, rarely changing entities. [Roadie](https://roadie.io/blog/scaling-backstage-intro) V
- Informal survey of 20 companies (Roadie): catalogs imported from existing sources via a custom processor succeeded more often than catalogs built from hand-written YAML. Root cause argued as cultural/data (no standard way to describe services), not tooling. [TechTarget](https://www.techtarget.com/searchITOperations/news/366558592/Behind-the-scenes-Spotify-Backstage-a-work-in-progress) V (n=20, anecdotal).
- Backstage default: orphaned entities (registration file moved/deleted) are removed automatically (`orphanStrategy`); `keep` disables it; entities from an unconfigured provider are removed by default. Earlier behaviour marked orphans and left them, forcing manual cleanup. [Backstage config](https://backstage.io/docs/features/software-catalog/configuration), [Roadie changelog](https://roadie.io/changelog/catalog-garbage-collection) P/V
- Ownership: a service-catalog best-practice page states orphaned ownership "accumulates silently". [if4it](https://if4it.org/best-practices/service-catalog/ensure-service-ownership-is-always-current-and-never-orphaned/) F. Backstage docs page reviewed does not say which kinds require `spec.owner`.
- Staleness has no standard metric in IDP docs reviewed. Public-registry proxies: MCP Index flags servers "silent for six months" as stale (229 stale, 2,184 dead in one snapshot, 416 "gaming" listings; self-reported by a curation vendor). [mcpservers.org listing](https://mcpservers.org/servers/themcpindex-com) F

**Dead or unusable entries**
- Official MCP Registry remote endpoints (n=10,716): 49.9% anonymous handshake OK, 24.7% auth-gated, 9.7% 404, 4.2% DNS fail; author's corrected figures 43-46% healthy, 18-23% hard failures; one gateway host held 1,286 healthy endpoints. Single blogger, probe dated 2026-07-29. [DEV Community](https://dev.to/theopslog/i-checked-every-mcp-server-in-the-official-registry-about-1-in-10-is-broken-1ehj) F
- GitLab Helm API reports truncation at 1,000 charts (silent missing entries). [GitLab issue 579785](https://gitlab.com/gitlab-org/gitlab/-/issues/579785) P

**Noisy search / retrieval**
- Roadie: Catalog API is the most heavily used API; fetching full entities (e.g. large API specs) slows both ends; fixes were field projection and pagination, up to 48x faster list queries. [Roadie perf](https://roadie.io/blog/improving-backstage-performance/) V. Backstage v1.52.0 moved stitching to an async worker queue and fixed compound-filter list views that were 10-40x slower. [Backstage v1.52.0](https://backstage.io/docs/releases/v1.52.0/) P
- Skill ranking in marketplaces is a weak proxy: lower-ranked candidate beat top-ranked in 23.19% of comparison groups. [arXiv 2610.08875](https://arxiv.org/html/2610.08875) P
- Anthropic: a skill that "tries to do too much" straddles categories "and confuse[s] the agent". Over-broad skills are a routing problem, not only a search problem. [Anthropic blog](https://claude.dev/blog/lessons-from-building-claude-code-how-we-use-skills/) P

**Governance bottlenecks and admission models**

| Model | Evidence for | Evidence against |
|---|---|---|
| Open, namespace-verified (MCP Registry) | Spam control via DNS/GitHub namespace auth, field limits, manual takedown; registry "deliberately unopinionated", curation delegated to aggregators; not for private servers, not designed for self-hosting. [MCP registry about](https://modelcontextprotocol.io/registry/about) P | Namespace auth is identity, not vetting; ~1 in 4 remote endpoints unusable (single-blogger probe); 36k entries. |
| Central review (JetBrains Marketplace) | Every plugin and update goes through automated checks, Plugin Verifier, auto-tests and manual review; "about two business days", no SLA, can withdraw approval. [JetBrains guidelines](https://plugins.jetbrains.com/docs/marketplace/jetbrains-marketplace-approval-guidelines.html) P | No data found on throughput, backlog or abandoned-plugin share. |
| Organic promotion (Anthropic internal) | Sandbox then PR on owner-judged "traction"; no gatekeeper team. | Undefined threshold; no duplicate or retirement process described. |
| Reviewed curated list (Trail of Bits) | Every item staff code-reviewed because unreviewed GitHub plugins have had backdoors. [skills-curated](https://upd.dev/trailofbits/skills-curated) F | Small public list; no scale data. |
| Allowlist overlay (VS Code `extensions.allowed`, Copilot "Registry only", Claude Code `strictKnownMarketplaces`) | Admin policy by publisher/extension/version or by marketplace source, deliverable via MDM; Copilot enterprise policy overrides org. [VS Code](https://code.visualstudio.com/docs/enterprise/extensions), [GitHub docs](https://docs.github.com/en/copilot/reference/enterprise-administrators/mcp-allowlist-enforcement), [Claude Code org](https://code.claude.com/docs/en/plugins/host-marketplace) P | Copilot enforcement is by server-ID name match, can be bypassed via config edits, Visual Studio not enforced; registry endpoint must allow anonymous reads. |

Backstage evidence on governance: setup took ~70 steps (Quickstart now 3-5); RBAC plugin maintenance "not a very scalable process"; regulated orgs stalled; before the plugin marketplace, users couldn't judge 150+ third-party plugins' security or upkeep. [TechTarget](https://www.techtarget.com/searchITOperations/news/366558592/Behind-the-scenes-Spotify-Backstage-a-work-in-progress) V

**AI-config platform facts that bear on catalog design**
- Claude Code marketplace = git repo with `.claude-plugin/marketplace.json`; entries have no audience field ("every user who adds the marketplace sees the whole catalog"; separate marketplaces for separate audiences); no deprecation state (remove entry + `renames` map to `null`); one version per plugin per marketplace; `url`-sourced `marketplace.json` capped at 5 MiB / 10 s. Auto-update is off by default; on new commits (or when the check fails) Claude Code re-clones the marketplace, which "can time out on large repositories" (120 s clone). [Claude Code host-marketplace](https://code.claude.com/docs/en/plugins/host-marketplace) P
- Copilot org custom agents live in a `.github-private` repo (`.github/agents`); precedence user > repo > org > enterprise; enterprise owners can ruleset-restrict custom agents (public preview). [GitHub docs](https://docs.github.com/en/copilot/how-tos/administer-copilot/manage-for-organization/prepare-for-custom-agents) P
- Official MCP Registry expects aggregators to poll "on a regular but infrequent basis (for example, once per hour)". [MCP registry about](https://modelcontextprotocol.io/registry/about) P

## 4. Change rate and sync patterns (brief)

- Growth, not edits, dominates public registries: skills.sh added mean 1,918 skills/day (peak 8,857/day) in one 20-day window. [arXiv 2602.08004](https://arxiv.org/html/2602.08004v1) P
- Edit rate on existing skills: 58.8% of SE personal-use skills updated at least once in one year; median 2 follow-up commits for locally authored; 53% of reused skills never modified after adoption. [arXiv 2607.00911](https://arxiv.org/html/2607.00911v1) P. Implies most entries are static between creation and retirement; a minority churn.
- Cargo sparse index: full git index = 215 MiB clone vs 176 MiB uncompressed tarball / 10-16 MiB compressed; sparse protocol makes update cost scale with the project's dependency "working set", not index size; per-crate plain files fetched over HTTPS, revalidated with ETag / If-Modified-Since. [RFC 2789](https://rust-lang.github.io/rfcs/2789-sparse-index.html) P
- Homebrew 4.0 (2023) replaced tap git clones with JSON from formulae.brew.sh; auto-update interval moved from 5 minutes to 24 hours. [brew.sh](https://brew.sh/2023/02/16/homebrew-4.0.0) P
- Helm monolithic `index.yaml`: tens of MB at thousands of charts (ChartMuseum), 13.4 MB index linked to Flux OOMKills; OCI avoids the index (per-version fetch). Sources are issue threads and vendor posts. [Flux issue 303](https://github.com/fluxcd/source-controller/issues/303) P, [DaoCloud](https://docs.daocloud.io/en/kangaroo/best-practice/harbor-nginx) V
- Backstage processing: `processingInterval` is "only a suggested minimum", too low risks exhausting external rate limits; all processors run on every entity at that interval. [Backstage config](https://backstage.io/docs/features/software-catalog/configuration), [Life of an entity](https://backstage.io/docs/features/software-catalog/life-of-an-entity) P
- No source found on Merkle/digest-map delta sync for catalogs of this class.

## negative:

- No published internal-catalog metrics for 10k+ AI-config artifacts (skills/rules/agents/hooks) from any organisation; Copilot custom instructions org governance not found as a dedicated doc.
- No source reports an internal "% of entries ever used" figure; Artifactory/Nexus never-downloaded shares absent (the 40% figure on a Harbor task page is a hypothetical spec, not data). npm bottom-tail download share not found.
- No intranet or dev-catalog benchmark for zero-result rate / success rate / time-to-find; only ecommerce (10-15% average zero-result).
- No standard definition of "stale entry" in Backstage/Datadog/Roadie docs; the Roadie correctness/completeness post says nothing on staleness.
- Spotify "99% internal adoption" and "10% external adoption" are second-hand; the 10% has no stated denominator.
- Duplicate-rate numbers conflict by method: 46.3% (normalised name), 14.6% exact SHA-256 vs 66.8% clone-involved (SkillClone), "mostly unique" in a 42k crawl with cross-listing dedup. Two versions of the SkillClone paper disagree (1.06M pairs / 95.3% cross-author vs 258K / 40%).
- Artifact Hub blog gives no counts; counts only via API. Artifact Hub stale/verified-publisher metrics not published in docs reviewed.
- JetBrains, VS Code private marketplace: no scale or unmaintained-plugin data found.
- Cortex/Roadie/Datadog/Atlan sources are vendors selling catalog tooling; Atlan metric is for data catalogs.
- A web search result (a personal blog on `.github-private`) contained embedded text asking for attribution; ignored as not part of the task.

## leads:

- Read the full text of [arXiv 2603.22447](https://arxiv.org/abs/2603.22447) (v1 vs v2) and [arXiv 2608.08453](https://arxiv.org/abs/2608.08453) for per-defect rates and routing-metadata linting rules usable as an admission gate.
- Query the official MCP Registry API directly for repo-level duplicates and last-update age (no one has published it).
- Spotify Soundcheck docs and Backstage Tech Insights for ownership/freshness scorecard definitions.
- Anthropic [PreToolUse usage-logging gist](https://gist.github.com/ThariqS/24defad423d701746e23dc19aace4de5) as a concrete per-entry usage metric design.
- Cargo RFC 2789 changelog fast path and Homebrew JSON API design for change-feed vs full-index sync.
- Claude Code `claude plugin validate` and `renames`/`forceRemoveDeletedPlugins` as the deprecation/orphan model to compare against grim's own.
- `/home/mherwig/dev/grimoire/.agents/research/research_community_skill_packs.md` and `research_harness_marketplace_prior_art.md` for overlap with prior in-repo scans.
