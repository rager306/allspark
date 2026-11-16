<p align="center">
  <a href="https://primeintellect.ai">
    <picture>
      <source media="(prefers-color-scheme: light)" srcset="https://github.com/user-attachments/assets/40c36e38-c5bd-4c5a-9cb3-f7b902cd155d">
      <source media="(prefers-color-scheme: dark)" srcset="https://github.com/user-attachments/assets/6414bc9b-126b-41ca-9307-9e982430cde8">
      <img alt="Prime Intellect" src="https://github.com/user-attachments/assets/6414bc9b-126b-41ca-9307-9e982430cde8" width="312" style="max-width: 100%;">
    </picture>
  </a>
</p>

<h3 align="center">
Prime Agent: A Self-Improving RLM Harness
</h3>

<p align="center">
  <a href="#install">Documentation</a> &bull;
  <a href="https://github.com/PrimeIntellect-ai/verifiers">Verifiers</a> &bull;
  <a href="https://github.com/PrimeIntellect-ai/prime-rl">PRIME-RL</a>
</p>

<p align="center">
  <a href="https://github.com/PrimeIntellect-ai/prime-agent/actions/workflows/ci.yml">
    <img src="https://github.com/PrimeIntellect-ai/prime-agent/actions/workflows/ci.yml/badge.svg" alt="CI" />
  </a>
  <a href="https://arxiv.org/abs/2608.23552">
    <img src="https://img.shields.io/badge/arXiv-2608.23552-b31b1b.svg" alt="arXiv" />
  </a>
</p>

<p align="center">
  <a href="https://trendshift.io/repositories/104249?utm_source=repository-badge&amp;utm_medium=badge&amp;utm_campaign=badge-repository-104249" target="_blank" rel="noopener noreferrer">
    <img src="https://trendshift.io/api/badge/repositories/104249" alt="PrimeIntellect-ai%2Fprime-agent | Trendshift" width="250" height="55" />
  </a>
</p>

---

## This is the allspark fork

This repository is the **allspark** fork of [PrimeIntellect-ai/prime-agent](https://github.com/PrimeIntellect-ai/prime-agent).
It is a downstream fork, not upstream: it adds seven fork-only crates — a native LSP tool, a
SQLite code graph, an edit engine, and the parsing, diff, memory and typed-judgment layers
under them (79,331 lines of Rust).

The binary deliberately keeps the upstream name `prime-agent`; the user-facing entry is the
`allspark` launcher (`scripts/allspark-launcher.sh`), which isolates all state under
`$HOME/.allspark-home` and never reads `~/.prime`.

**[`FORK.md`](FORK.md)** maps the fork in one page: the crates, the tag scheme, the entry
points, and where to read next. Everything below this banner documents upstream Prime Agent,
and all of it still applies to the fork.

### Installing and updating the fork

The commands below are the **only** supported way to install or update allspark. Upstream's
installer (`app.primeintellect.ai/.../install.sh`) is not one of them: it installs Prime Agent
without the fork crates.

#### Install

```bash
curl -fsSL https://raw.githubusercontent.com/rager306/allspark/main/install-rust.sh | sh
```

That is safe to run as-is. The published script carries the fork's own release URL as its
built-in default, so it selects the **fork layout** on its own:

| | path |
|---|---|
| payload | `~/.local/libexec/prime-agent` |
| launcher | `~/.local/bin/allspark` |
| user settings, sessions, auth | `$HOME/.allspark-home` (never `~/.prime`) |

The upstream paths (`~/.local/share/prime-agent`, `~/.local/bin/prime-agent`) are **left
untouched**, so an installed upstream Prime Agent keeps working beside the fork.

If that reports `could not resolve the latest beta version`, the fork's release channel has
not been cut yet: the newest published release predates the channel (it carries no `beta`
pointer and names its tarball `allspark-v<version>-linux-x86_64` instead of the channel's
`prime-agent-<version>-linux-x64`), so neither the channel install nor a `PRIME_AGENT_VERSION`
pin can resolve it. Install that release by hand instead:

```bash
V=0.10.2-beta.3-fork.010   # the tag's allspark-v prefix stripped
curl -fsSLO "https://github.com/rager306/allspark/releases/download/allspark-v${V}/allspark-v${V}-linux-x86_64.tar.gz"
tar -xzf "allspark-v${V}-linux-x86_64.tar.gz"
# the archive already holds the fork layout: libexec/prime-agent + bin/allspark
mkdir -p ~/.local && cp -a "allspark-v${V}-linux-x86_64/." ~/.local/
~/.local/bin/allspark --version
```

Once a release cut from the fixed workflow exists, the one-liner above installs the current
channel and `allspark update` works.

#### Update

```bash
allspark update
```

If it reports `404` for an installer URL, the release you are on predates the fork's update
channel; re-run the install command above instead. Do not run `prime-agent update` — that is the
upstream binary's own updater and would replace the fork payload.

#### Upgrade from a manual or archived install

Read the version from the release's `stable` pointer, then fetch that tarball:

```bash
VERSION="$(curl -fsSL https://github.com/rager306/allspark/releases/latest/download/stable)"
curl -fsSL -o /tmp/allspark.tar.gz \
  "https://github.com/rager306/allspark/releases/latest/download/prime-agent-${VERSION}-linux-x64.tar.gz"
sh install-rust.sh --archive /tmp/allspark.tar.gz
```

#### Uninstall

```bash
rm -rf ~/.local/libexec/prime-agent ~/.local/bin/allspark ~/.allspark-home
```

#### What NOT to do

| don't | why |
|---|---|
| `curl … \| sh` with upstream's URL | installs Prime Agent without the fork crates |
| `prime-agent update` | the upstream payload's updater; replaces the fork binary |
| `allspark update` on a pre-channel release | known: the channel assets did not exist; re-install instead |
| copy `~/.prime` into `~/.allspark-home` | rotating OAuth refresh tokens invalidate each other |

#### Checking what you have

```bash
allspark --version
# allspark 0.10.2-beta.10 (fork build fork.012; build f541ea75f94c; upstream prime-agent 0.10.2-beta.10)
```

`--version` names the fork, the fork build, the content hash, and the upstream base it merged.
A plain `0.10.2` with no `(fork build …)` is an upstream install, not this one.

---

Prime Agent is an open-source coding and research agent for general and long-running work. It is designed around two core abstractions:

- The **[Recursive Language Model (RLM)](https://www.primeintellect.ai/blog/rlm)** treats context as variables (*prompt-as-a-variable*) and tools like recursive subagents as function calls (*programmatic tool /sub-agent calling*) inside a persistent REPL.
- The **[Continual Harness](https://arxiv.org/abs/2605.09998)** stores supplemental prompts, memories, skill descriptions, and reusable subagent specifications as durable state that Prime Agent can refine through small, evidence-backed updates, local to the session by default.

## Install

Install the latest stable version with the one-command installer:

```bash
curl -fsSL https://app.primeintellect.ai/prime-agent/install.sh | sh
```

On Windows, install from PowerShell:

```powershell
irm https://app.primeintellect.ai/prime-agent/install.ps1 | iex
```

The Windows installer downloads the latest stable Windows x64 build and verifies its checksum before installation.

## Why Prime Agent

Prime Agent combines a persistent Python control environment with durable harness state, so useful working context and reusable operating patterns can outlive a single chat window.

- **Everything is programmatic:** a persistent Python REPL is the built-in model tool; file operations, shell commands, tool use, subagents, and context management happen through code.
- **Subagents are built in:** `rlm.spawn(...)` spawns real child agents for parallel or background work and returns their results programmatically.
- **The harness can improve:** `/refine` reviews the current trajectory and can apply small, evidence-backed updates to supplemental harness state. It never rewrites the immutable base system prompt, and recorded snapshots support rollback.
- **Skills can be executable:** Python skills in your user configuration are imported into the kernel; repository skills provide Markdown instructions without automatic code execution.
- **Sessions run in the background:** daemon-backed agents keep running when the terminal disconnects and can be reattached later.
- **Agents communicate directly:** running agents can exchange messages and orchestrate one another without routing everything through the user.
- **Long tasks keep moving:** automatic compaction, persistent goals, heartbeats, schedules, autonomous mode, and retained subagents preserve progress across turns and terminal sessions.

## Getting Started

Start Prime Agent from the repository or directory you want it to work in:

```bash
cd /path/to/project
prime-agent
```

On first launch, run `/login` to choose a subscription or API-key provider. Prime Agent works in the current directory and can run commands and modify files there. Use a disposable clone, clean worktree, or another checkpoint you can inspect and restore.

> [!WARNING]
> Prime Agent executes model-generated Python and project commands with your user permissions. Its worker and kernel processes improve lifecycle isolation and recovery; they are **not** a security sandbox. Review changes and use trusted repositories, instructions, and skills only. Run untrusted code or instructions in an external sandbox or restricted environment.

Useful commands:

```bash
prime-agent agents                   # Browse running, idle, and saved sessions
prime-agent attach <agent>           # Reattach to a running session
prime-agent --resume [path|id]       # Browse sessions or resume one directly
prime-agent status                   # Inspect background service state
prime-agent doctor [--fix]           # Inspect or repair background services
prime-agent update [--force]         # Update Prime Agent
prime-agent shutdown [--force]       # Stop every agent, worker, and background service
```

## Built for Long-Running Work
Prime Agent is built for long-running work, especially for evaluations in research. These features are available in the TUI, and when run autonomously.

- **Continual Harness:** `/refine` can persist focused, reviewable lessons as supplemental prompts, memories, reusable skill descriptions, or subagent specifications, with recorded refinement history. It does not replace packaging and reviewing new executable skills.
- **Direct agent-to-agent communication:** running agents and retained subagents can discover one another, exchange messages, and steer active work.
- **Daemon-backed continuity:** active sessions, Python REPL state, schedules, and subagents keep running when the terminal detaches and can be reattached later.
- **Heartbeats and schedules:** `/heartbeat`, `rlm_heartbeat`, and `prime-agent schedule` can re-enter a session periodically or at a specific time.
- **Persistent goals:** `/goal` keeps an objective and its progress active across turns until it is completed, paused, or cleared.
- **Bounded autonomous mode:** `/autonomous` continues within configured turn, token, and time budgets and can run user-defined quality gates. A passed gate checks only what that gate verifies; reaching a limit does not imply task success.

## Contributing

Start with a GitHub Discussion for [general questions](https://github.com/PrimeIntellect-ai/prime-agent/discussions/categories/general), [bug reports](https://github.com/PrimeIntellect-ai/prime-agent/discussions/categories/bug-reports), and [feature requests](https://github.com/PrimeIntellect-ai/prime-agent/discussions/categories/feature-requests). Maintainers promote accepted work into Issues, and pull requests are reviewed from maintainers and vouched contributors.

## License

Prime Agent is fully open source and released under the [MIT License](LICENSE).

## Citation

If you use this codebase in your research, please cite Prime Agent:

```bibtex
@article{karten2026prime,
  title={Prime Agent: A Self-Improving RLM Harness},
  author={Karten, Seth and Zhang, Alex L. and Thomas, Kevin and Müller, Sebastian and Bakouch, Elie and Auras, Daniel and Senghaas, Mika and Obeid, Fares and Dunas, Konstantin and Hagemann, Johannes and Jaghouar, Sami},
  journal={arXiv preprint arXiv:2608.23552},
  year={2026}
}
```

Available at [https://arxiv.org/abs/2608.23552](https://arxiv.org/abs/2608.23552).
