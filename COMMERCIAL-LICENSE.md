# LVQR Commercial License

LVQR is **dual-licensed**: AGPL-3.0-or-later for open-source
use, commercial terms for everyone else.

## Which license applies to you

**You MAY use LVQR under the AGPL-3.0-or-later** (see
[`LICENSE`](LICENSE)) if:

* You are a non-commercial user (personal projects, research,
  education, non-profits, hobbyists), or
* You are a commercial entity AND you are willing to release
  the complete source code of every program you distribute or
  operate as a network service that incorporates, links to, or
  communicates with a modified version of LVQR, under AGPL
  terms, including every proprietary module you build on top
  of LVQR.

AGPL-3's copyleft is **network copyleft**: running LVQR as a
hosted service counts as "distribution" for license purposes,
so if you SaaS-host a product built on LVQR you must publish
your full source under AGPL too.

**You MUST buy a commercial license** if:

* You want to build a proprietary product on top of LVQR
  without open-sourcing your code, or
* You want to offer LVQR (or a modified version) as a managed
  or hosted service without publishing your infrastructure and
  product code under AGPL, or
* You need indemnification, warranty, priority security
  response, or any other terms AGPL does not provide.

## How to obtain a commercial license

Email **hackbuildvideo@gmail.com** with:

* Your company name and a point of contact.
* A short description of what you are building and how LVQR
  fits in (one paragraph is fine).
* Expected deployment scale (single-node, cluster, number of
  concurrent broadcasts, whether you plan to redistribute the
  binary or host as a service).

A commercial license grants you the same code under permissive
terms (no copyleft obligations), with per-deployment or
per-organisation pricing scaled to your usage and willingness
to support the project.

## Why dual-license

LVQR is aiming to compete with commercially-backed live video
servers (AWS Kinesis Video Streams, LiveKit Cloud, Ant Media
Enterprise). The open-source user base drives adoption and
quality; the commercial license funds the full-time engineering
needed to keep LVQR at production grade without venture
capital pressure to exit.

This is the same model MongoDB (pre-SSPL), MySQL (pre-Oracle),
MariaDB, Sentry, GitLab, Mattermost, and many others use.

## Contributing

Contributions are accepted under the project's AGPL terms. By
submitting a pull request you agree that your contribution is
licensed under AGPL-3.0-or-later and also grant the project
maintainer (Moheeb Zara, hackbuildvideo@gmail.com) a
perpetual, irrevocable, worldwide license to relicense your
contribution under the commercial terms above. This is the
mechanism that keeps the dual-license model honest: every line
in the repo is either owned by the maintainer or explicitly
relicenseable.

If you cannot agree to those terms for a specific contribution
please mention it in your pull request and we will discuss
alternatives.

## Frequently asked questions

**Can I use LVQR internally at my company without buying a
commercial license?**
If you run it purely internally (no SaaS, no distribution of a
derivative to third parties), AGPL only triggers if you modify
the source AND either distribute the modified binary or host
the modified version as a network service available to users
outside your organisation. An internal deployment you operate
for your own employees is typically fine under AGPL; check
with legal counsel for your jurisdiction.

**Can I embed LVQR in a proprietary product I ship to
customers?**
No, not under AGPL. Either open-source your product under AGPL
or buy a commercial license.

**Can I build a SaaS on top of LVQR?**
Under AGPL, yes, but your entire SaaS product (including
proprietary modules) must be open-sourced under AGPL. If that
is not acceptable, buy a commercial license.

**What about forks and derivative open-source projects?**
Welcome. They must stay AGPL-3.0-or-later under the copyleft.
No additional license required.

**What counts as a "modification"?**
Writing a WASM filter (Tier 4 item 4.2), an in-process AI
agent (Tier 4 item 4.5), a custom ingest protocol, or any
changes to LVQR source code all count. Running the stock
binary with configuration files, CLI flags, or environment
variables does NOT count as modification.

**What about the example filters in
`crates/lvqr-wasm/examples/`?**
Those are AGPL-licensed like the rest of the repo. A filter
you compile from your own source and drop in via
`--wasm-filter` is not a derivative of LVQR at the source
level -- it runs in a sandboxed wasmtime instance and only
communicates via a narrow ABI -- and is therefore not subject
to AGPL copyleft on its own source. See FSF's guidance on
plugin boundaries for the reasoning.

## Legal note

This document is a summary; the [`LICENSE`](LICENSE) file is
the authoritative AGPL text. If anything here disagrees with
the GNU Affero General Public License, the license text
wins.

For commercial-license negotiations, contracts replace this
document entirely. Email hackbuildvideo@gmail.com to start the
conversation.
