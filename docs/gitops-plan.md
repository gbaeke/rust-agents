# Plan: a GitOps version of the Kubernetes setup, in a new repo

Status: **not started.** This plan is written so it can be picked up from scratch in a later session.

This repo deploys with scripts: `kind/cluster.sh`, `kind/agentgateway.sh` and `deploy/deploy.sh`. The new repo deploys the same app onto the same kind setup, but [Argo CD](https://argo-cd.readthedocs.io/) applies everything from git. Both versions stay, so they can be compared side by side. The new repo is created from scratch and is **self-contained**: the Rust source (`agent/`, `mcp-server/`, `knowledge-base/`, `Cargo.toml`, `Cargo.lock`, `Dockerfile`), the manifests and the cluster config are all copied from here. After that, neither repo depends on the other, and they can drift apart.

## Goal

| Phase | Outcome |
|---|---|
| **1. Cluster, registry and Argo CD** | A second kind cluster `gitops` with a local image registry, Argo CD installed by one script, and a root Application that syncs the repo. |
| **2. Platform from git** | Gateway API CRDs, agentgateway (CRDs and controller) and Sealed Secrets, each an Argo Application, installed in the right order with no script. |
| **3. The app from git** | rust-agents (agent, MCP server, Postgres, gateway, network policies) synced by Argo, including secrets and the `agentgateway` database. It behaves exactly like the script version. |
| **4. Day-to-day flow** | A new image is built and released by changing its tag in git. Drift is undone automatically, and a rollback is a `git revert`. |
| **5. Many apps** | An ApplicationSet turns each folder under `apps/` into an app in its own namespace, limited by an AppProject. A second small app proves it. |

Each phase ends with a checkpoint where the user reviews the result before the next one starts.

## Rules carried over from this repo

- No sudo: when a command needs it, give the user the command.
- The Anthropic key never goes in git (not even encoded) or in process arguments. Read it from a file and pass it on stdin.
- Confirm before creating the GitHub repo and before every push.
- Tools come from mise. Don't commit someone else's `mise.toml`.
- The admin UI pages `/ui/llm/analytics` and `/ui/llm/logs` are unlisted (see this repo's `CLAUDE.md`). Copy that note into the new repo's `CLAUDE.md`.

## Decisions (proposed; confirm at the start)

| # | Question | Proposal | Why / alternative |
|---|---|---|---|
| D1 | Repo name and visibility | `gbaeke/rust-agents-gitops`, **public** | Argo then needs no repo credentials. Only encrypted secrets are in git. If it's private, Argo needs a deploy key, which becomes one more secret to create at bootstrap. |
| D2 | Where images come from | A **local registry** container `kind-registry` (host port 5001), built from the repo's own source by `scripts/release.sh` | CI in the same repo pushing to ghcr.io is a possible later step. Its tag-bump commit must not trigger another build, so filter the workflow on source paths. |
| D3 | Image tags | The short hash of the last commit that touched the source (`git log -1 --format=%h -- agent mcp-server knowledge-base Cargo.toml Cargo.lock Dockerfile`). Refuse to release while those paths have uncommitted changes. | Argo only reacts to changes in git, so a reused `:dev` tag would never roll out. The release commit itself only changes `apps/`, so it doesn't change the tag. |
| D4 | Secrets in git | **Sealed Secrets.** The sealing key is backed up outside git and restored before the controller starts. | Without the backup, every `cluster.sh down` and `up` means sealing everything again. External Secrets Operator needs a secret store, so it's overkill for kind. |
| D5 | Second cluster or the same one | A **second cluster `gitops`**, next to `dev` | The script and GitOps versions don't fight over one namespace. The cost is about 1.3 GiB more RAM, and the machine has 27 GB. |
| D6 | Ports | Host ports **31000–31009** → NodePorts 30000–30009 | Apps keep the same NodePorts as on `dev`. ufw-docker matches the container port, so the existing `ufw route allow … 30000:30009` should cover it. Verify. The agent is at `http://omarchy:31000`, and the Argo UI at `http://omarchy:31009`. |
| D7 | Repo layout | App-of-apps with one root, below | It's the common pattern and easy to follow in the Argo UI. |
| D8 | Who manages Argo itself | Helm installs it once at bootstrap. Argo managing itself is optional, in phase 4. | Self-management is neat, but a mistake there can break Argo. |

## Repo layout

```
rust-agents-gitops/
  README.md                 what this is, the comparison with the script version, quick start
  CLAUDE.md                 UI page note, rules above
  docs/plan.md              this plan, updated as phases complete
  Cargo.toml, Cargo.lock    the workspace, copied from rust-agents
  Dockerfile                same multi-stage build: targets agent and mcp-server
  agent/, mcp-server/       Rust source, copied
  knowledge-base/           baked into the agent image, copied
  cluster/
    cluster.yaml            kind config: 1 control-plane + 2 workers, ports 31000-31009, registry mirror
    cluster.sh              install | up | start | stop | status | down (also starts the registry)
  bootstrap/
    bootstrap.sh            restore the sealing key, helm install Argo CD, apply root.yaml
    argocd-values.yaml      NodePort 30009, insecure (http), health checks (see phase 1)
    root.yaml               the root Application → argocd/
  argocd/                   what the root syncs: only Argo objects
    platform.yaml           Applications: gateway-api-crds, agentgateway-crds, agentgateway, sealed-secrets
    projects.yaml           AppProject `platform`, AppProject `apps`
    apps.yaml               the ApplicationSet over apps/*
  platform/
    gateway-api/kustomization.yaml   remote standard-install.yaml at the pinned version
  apps/
    rust-agents/            kustomize: the manifests from deploy/k8s plus sealed secrets and the db Job
    hello/                  phase 5: a tiny second app
  scripts/
    seal.sh                 makes apps/rust-agents/sealed-secrets.yaml from the key in a .env file
    release.sh              build and push images, set tags, commit (push after confirming)
    sealing-key.sh          backup | restore of the Sealed Secrets key (to ~/.config/rust-agents-gitops/)
```

---

## Phase 1: cluster, registry and Argo CD

1.1 **Create the repo.** After confirming D1, run `gh repo create`, then:
- copy the Rust source, as listed in the layout above, from this repo at its current `main`. Note the source commit in the first commit message. Leave out `compose.yaml`, `kind/`, `deploy/` and `target/`;
- check that `cargo build` and `docker build --target agent` work there;
- add the README skeleton, this plan as `docs/plan.md`, and `.gitignore` (`target/`, `.env`, `*.key`, `*-key.yaml`).

1.2 **`cluster/cluster.yaml` and `cluster.sh`.** Start from this repo's `kind/` and change:
- the name, with `KIND_CLUSTER` defaulting to `gitops`;
- the port mappings, from host 31000–31009 to container 30000–30009;
- a `containerdConfigPatches` entry for `/etc/containerd/certs.d`, so the nodes can pull `localhost:5001/...` from the registry. Follow kind's "local registry" guide.

`cluster.sh up` also starts the `kind-registry` container (`registry:2`, `127.0.0.1:5001→5000`) if it isn't running. It connects the container to the `kind` network, writes `hosts.toml` into each node, and applies kind's `local-registry-hosting` ConfigMap. `install` also installs the `argocd` and `kubeseal` CLIs through mise. `down` leaves the registry running, since another cluster may use it.

1.3 **`bootstrap/bootstrap.sh`:**
1. Check that the context `kind-gitops` exists.
2. If a sealing key backup exists, restore it into `kube-system` before Sealed Secrets starts (see 2.4).
3. `helm upgrade -i argocd argo/argo-cd -n argocd --create-namespace -f argocd-values.yaml --version <pinned>`. Pin the latest stable chart, checked at the start.
4. Wait for the server.
5. `kubectl apply -f root.yaml`.
6. Print the UI URL and the admin password (`argocd-initial-admin-secret`).

Subcommands: `password`, `status`, `uninstall`.

1.4 **`argocd-values.yaml`:**
- `server.service.type: NodePort` with nodePort 30009, and `configs.params.server.insecure: true`. It's plain http on a lab machine; say so in the README.
- **A health check for `argoproj.io/Application`.** Since Argo CD 1.8 a parent doesn't wait for its child Applications to be healthy. Without this check, the platform's sync waves don't order anything. Use the Lua snippet from the Argo docs ("App of Apps" / sync waves).
- Health checks for `gateway.networking.k8s.io/Gateway` (condition `Programmed=True`) and `agentgateway.dev/AgentgatewayModel` / `AgentgatewayBackend` (`Accepted=True`). Check first whether the pinned Argo already has them built in; if not, add Lua under `resource.customizations.health.<group>_<kind>`.
- Skip dex and notifications to save memory.

1.5 **`root.yaml`:** an Application for `argocd/` on `main`, with `automated: {prune: true, selfHeal: true}`.

**Checkpoint 1:** the Argo UI opens at `http://omarchy:31009`, the root Application is Synced and Healthy (with nothing under it yet), and the registry answers. Test that by tagging and pushing a `busybox` image and running a pod from `localhost:5001/busybox`. Record how much memory Argo uses.

---

## Phase 2: the platform from git

2.1 **AppProject `platform`:** may create cluster-wide resources (CRDs, ClusterRoles, webhooks) and deploy to `agentgateway-system`, `kube-system` and `argocd`. Apps don't get this project.

2.2 **Applications in `argocd/platform.yaml`,** in sync waves:

| Wave | Application | Source | Notes |
|---|---|---|---|
| -3 | `sealed-secrets` | Helm chart `sealed-secrets` (bitnami-labs repo), pinned | `fullnameOverride: sealed-secrets-controller` in `kube-system`. That's the name and place `kubeseal` expects by default. |
| -3 | `gateway-api-crds` | `platform/gateway-api` (kustomize, remote `standard-install.yaml` v1.6.3) | `ServerSideApply=true`: the CRDs are too large for a client-side apply. |
| -2 | `agentgateway-crds` | OCI Helm `cr.agentgateway.dev/charts/agentgateway-crds` v1.7.0 | `ServerSideApply=true`. In Argo the OCI repo is written without `oci://` and with `enableOCI`. Check the current syntax. |
| -1 | `agentgateway` | OCI Helm `cr.agentgateway.dev/charts/agentgateway` v1.7.0 | Namespace `agentgateway-system`, `CreateNamespace=true`. Same values as `kind/agentgateway.sh` (none so far). |

2.3 **The GatewayClass.** Check that the controller chart creates `agentgateway` as it does now, and that Argo doesn't show it as OutOfSync or prune it. It's created by the chart, so it's tracked.

2.4 **`scripts/sealing-key.sh`:**
- `backup` saves the Secret labelled `sealedsecrets.bitnami.com/sealed-secrets-key` from `kube-system` to `~/.config/rust-agents-gitops/sealing-key.yaml`, mode 600.
- `restore` applies that file, and `bootstrap.sh` runs it first.

Run `backup` once, right after the controller first starts.

**Checkpoint 2:** all four Applications are Synced and Healthy, and `kubectl get gatewayclass agentgateway` is Accepted. Then test the order: run `cluster.sh down`, `up` and `bootstrap.sh`, and check that the platform comes back with no manual step and the same sealing key (compare `kubeseal --fetch-cert` before and after).

---

## Phase 3: the app from git

3.1 **Copy the manifests.** Copy `deploy/k8s/*` from this repo into `apps/rust-agents/` without changes, then adapt:
- **Images:** `localhost:5001/rust-agents/agent` and `localhost:5001/rust-agents/mcp-server`, tags set through `images:` in `kustomization.yaml`, and `imagePullPolicy: IfNotPresent`. That's fine now that tags are never reused.
- **Remove** the comments that refer to `deploy.sh`, and point them at the new scripts.

3.2 **Secrets.** `scripts/seal.sh <path to .env>` builds both Secrets with `kubectl create secret generic --dry-run=client -o yaml --from-env-file=/dev/stdin` and pipes them into `kubeseal -o yaml`. It writes `apps/rust-agents/sealed-secrets.yaml`. Nothing unencrypted touches the disk or a process argument.
- `anthropic`: `Authorization=<key>`, read from the `.env` line as `deploy.sh` does.
- `postgres`: `password=$(openssl rand -hex 24)`, generated **only if** the sealed file doesn't have it yet, because a new password would lock Postgres out of its existing data.

A SealedSecret is encrypted for its namespace and name (the default strict scope). Keep both as they are.

3.3 **The `agentgateway` database.** Add `db-init-job.yaml`, an Argo `Sync` hook (`hook-delete-policy: BeforeHookCreation`) running `postgres:17` with `psql`:
- `SELECT 1 FROM pg_database WHERE datname='agentgateway'`, then `CREATE DATABASE agentgateway` if it's missing. It's safe to run on every sync.
- `PGPASSWORD` comes from Secret `postgres`.
- It must pass the restricted profile: runAsNonRoot uid 999, seccomp RuntimeDefault, drop ALL, no privilege escalation.
- Add a NetworkPolicy check: the current policies only restrict the agent and MCP pods, so the Job can reach Postgres. Keep it that way.

3.4 **Sync waves for the resources** (annotation `argocd.argoproj.io/sync-wave`):

| Wave | Resources |
|---|---|
| -2 | Namespace (with the restricted labels) |
| -1 | SealedSecrets, the `agent-config` ConfigMap |
| 0 | Postgres StatefulSet and Service |
| 1 | db Job (hook) |
| 2 | Gateway, AgentgatewayParameters, AgentgatewayModel, AgentgatewayBackend, HTTPRoute |
| 3 | mcp-server, agent, PDBs, NetworkPolicies |

The point is that the agent never starts before the gateway proxy exists, the same order `deploy.sh` waits for.

3.5 **The Application.** For now, add a plain Application `rust-agents` in `argocd/` (the ApplicationSet replaces it in phase 5), in project `apps`, destination `rust-agents`. Don't use `CreateNamespace`: the namespace comes from git, with its security labels.

3.6 **Things to watch for:**
- **Controller-made objects.** The proxy Deployment and Service `agentgateway` are made by the controller and must not be pruned or shown as OutOfSync. Check how Argo treats them: they carry an ownerReference to the Gateway, so they should show as its children.
- **Fields filled in after apply.** Anything on AgentgatewayParameters or Gateway that the API server or controller fills in, causing a permanent OutOfSync, gets `ServerSideApply=true` on the Application or a narrow `ignoreDifferences`.
- **The Postgres PVC** comes from `volumeClaimTemplates`, so Argo doesn't own it and deleting the Application keeps the data. Document this, and give `scripts/` a `wipe` command that deletes the namespace after confirming.

3.7 **First release.** `scripts/release.sh`:
1. Refuse if the source paths have uncommitted changes. The tag is as in D3.
2. `docker build --target agent` and `--target mcp-server` from the repo root.
3. Push to `localhost:5001`.
4. `kustomize edit set image` (or `kubectl kustomize` tooling) in `apps/rust-agents`.
5. Commit "Release rust-agents <sha>".
6. Show the diff, and push only after confirming.

Then `argocd app sync rust-agents` or a refresh, so we don't wait for the 3-minute poll.

**Checkpoint 3:** repeat the checks done on the script version (see `docs/kubernetes-plan.md`):
- 3 agent pods, and a chat that continues across pods and after every pod is deleted;
- 409 when busy;
- reset;
- an undeclared model refused (404);
- direct egress blocked;
- the request log rows in the analytics page.

Then run `cluster.sh down`, `up` and `bootstrap.sh`, and check that the whole app comes back from git with no other step. Postgres data is lost then, since the PVC goes with the cluster. That's expected.

---

## Phase 4: day-to-day flow

4.1 **A release:** change code in this repo, commit, `release.sh`, push, sync. Agent pods roll and nothing else changes.

4.2 **A setting:** change `SESSION_TTL_DAYS` in git and push. The ConfigMap hash changes, the pods roll, and the old ConfigMap is pruned.

4.3 **Drift:** `kubectl scale deploy/agent --replicas=1` is reverted by selfHeal within seconds. `kubectl delete networkpolicy agent-egress` comes back.

4.4 **Rollback:** `git revert` the release commit and push, and the old images come back. Compare this with Argo's own History/Rollback, which is disabled while auto-sync is on.

4.5 **Inner loop.** Document how to iterate fast without a commit per edit:
- `argocd app set rust-agents --sync-policy none`, then `kubectl set image` or `kind load`;
- re-enable auto-sync and release properly when done.

4.6 **Optional:** Argo manages itself. An Application `argocd` for the same chart and values in project `platform`. Only do this if the user wants it (D8).

**Checkpoint 4:** write in the README a short comparison with the script version: what each step costs, what git now records, and what still needs a script (the cluster, the bootstrap, sealing, images).

---

## Phase 5: many apps

5.1 **ApplicationSet `apps`.** A git directory generator over `apps/*`:
- each directory gives an Application named after it, deployed to the namespace with that name, in project `apps`;
- the same auto-sync and prune settings;
- it replaces the hand-made Application from 3.5. Watch that the change doesn't delete and recreate the app's resources: use the same Application name, and check `preserveResourcesOnDeletion` before switching.

5.2 **AppProject `apps`:**
- **Cluster-wide resources:** only `Namespace`, so no CRDs, ClusterRoles or webhooks. Apps use the platform; they don't change it.
- **Destinations:** this cluster, any namespace except `kube-system`, `argocd` and `agentgateway-system`. Check that the pinned Argo supports `!` exclusions in destinations; if not, list the app namespaces.
- **Source:** only this repo.

5.3 **App `hello`.** Add `apps/hello/`:
- a namespace with the restricted profile;
- a tiny Deployment (`nginxinc/nginx-unprivileged` or a static Go/Rust image) on NodePort 30001 → `http://omarchy:31001`.

Adding the folder and pushing is the only step. Then show that `hello` can't create a ClusterRole: add one, watch the sync fail with a project error, and remove it.

**Checkpoint 5:** two apps, each from its folder, each limited to its namespace. Removing a folder removes its app.

---

## Open questions to check while building

- The current Argo CD and Sealed Secrets chart versions, and the exact OCI Helm source syntax for agentgateway charts.
- Whether Argo has built-in health checks for Gateway API and agentgateway resources (1.4).
- Whether kind's registry setup still uses `certs.d` / `hosts.toml` on the kind version from mise (0.33).
- Whether the existing ufw-docker rule covers the new host ports (D6).
- Memory with both clusters and Argo running.
