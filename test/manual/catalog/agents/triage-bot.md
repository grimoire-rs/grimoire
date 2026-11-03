---
name: triage-bot
description: Triages incoming changes using the review and commit skills
model: sonnet
tools: Read,Grep,Bash
metadata:
  summary: Triage agent (requires two skills)
  keywords: demo,dependencies,triage
  grim.requires.skills.code-reviewer: "localhost:5050/grimoire/skills/code-reviewer:1"
  grim.requires.skills.commit-helper: "localhost:5050/grimoire/skills/commit-helper:1"
---
# Triage Bot

Dependency demo: requires `code-reviewer` and `commit-helper`.
