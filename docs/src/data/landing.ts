// Single source of truth for the landing page's entry-path tiles and
// pain-router cards. Consumed by WaysIn.astro, PainRouter.astro, and
// FooterNav.astro in a later work package.

export interface EntryPath {
  label: string;
  href: string;
  blurb: string;
}

export interface RouterCard {
  pain: string;
  href: string;
  taskIds: string[];
}

export const entryPaths: EntryPath[] = [
  {
    label: "Quick start (CLI)",
    href: "/quickstart.html",
    blurb: "Install grim, add one published skill, watch your agent list it.",
  },
  {
    label: "Browse the index",
    href: "/browse.html",
    blurb: "On the website, in the TUI, or from VS Code: pick from the catalog, install it, read the badges. Then narrow what your team sees.",
  },
  {
    label: "Your first skill",
    href: "/first-skill.html",
    blurb: "A SKILL.md in a folder of your repo, rendered into every agent that works on it. Then one link to share it.",
  },
];

export const routerCards: RouterCard[] = [
  {
    pain: "the same skill, rule or MCP server has to live in .claude, .cursor and .agents and drifts",
    href: "/guides/shared-skills.html",
    taskIds: ["T31", "T04", "T19"],
  },
  {
    pain: "project or global, which agents, and a repo skill leaking everywhere",
    href: "/guides/scopes-and-clients.html",
    taskIds: ["T03", "T07"],
  },
  {
    pain: "installing a stranger's skill without knowing what it does",
    href: "/guides/inspect.html",
    taskIds: ["T14", "T15"],
  },
  {
    pain: "the catalog shows everything from every registry; my team should see only ours",
    href: "/guides/registries.html",
    taskIds: ["T06"],
  },
  {
    pain: "a sync tool rewrote the file I edited",
    href: "/guides/lifecycle.html",
    taskIds: ["T08", "T13", "T17"],
  },
  {
    pain: "which reference moves on update and which never does",
    href: "/guides/versioning.html",
    taskIds: ["T28"],
  },
  {
    pain: "CI cannot prove the reviewed skills are the ones that shipped",
    href: "/guides/team-ci.html",
    taskIds: ["T09"],
  },
  {
    pain: "private skills have nowhere to live but per-repo copies",
    href: "/tutorials/own-index.html",
    taskIds: ["T10", "T21", "T27", "T29"],
  },
];
