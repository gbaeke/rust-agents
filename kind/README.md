# Local Kubernetes cluster (kind)

A local [kind](https://kind.sigs.k8s.io/) cluster that any app can run on: one control-plane node and two workers, each a Docker container. Nothing in this folder knows about a specific app, so you can copy it to another repo or move it into a repo of its own.

```sh
kind/cluster.sh install   # kind, kubectl, helm (through mise, or pacman if there is no mise)
kind/cluster.sh up        # create the cluster, or start it if it was stopped
kind/cluster.sh status    # nodes and the NodePorts that apps use
kind/cluster.sh stop      # stop the nodes and keep all state; `start` or `up` brings them back
kind/cluster.sh down      # delete the cluster
```

The cluster is called `dev` and the kubectl context is `kind-dev`. Set `KIND_CLUSTER=name` to run a second cluster next to it. Each cluster also needs its own host ports in `cluster.yaml`.

## Reaching apps from another machine

Host ports **30000–30009** go straight to the same NodePorts. If an app's Service has `type: NodePort` and `nodePort: 30000`, the app is at `http://<host>:30000`. kind can only map ports when it creates the cluster, so to add more, edit `cluster.yaml`, then run `down` and `up`.

With ufw-docker, open the range once:

```sh
sudo ufw route allow proto tcp from any to any port 30000:30009
```

## Getting an app's images in

The nodes can't see local Docker images. Load them after every build, with no registry needed:

```sh
kind load docker-image my-app:dev --name dev
```

Use a tag other than `:latest` and set `imagePullPolicy: IfNotPresent`. With `:latest`, Kubernetes tries to pull the image from Docker Hub.

## Pod security

Give every app its own namespace and enforce the `restricted` Pod Security Standard on it:

```yaml
metadata:
  labels:
    pod-security.kubernetes.io/enforce: restricted
```
