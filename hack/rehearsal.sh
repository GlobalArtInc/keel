#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
root=$PWD

product=dns
binary=bin/keel
reachable=(keel keel-host keel-system)
source_url=${1:-}
revision=${2:-}
tmp=""
refused=""
cargo_home=""

usage() {
  cat >&2 <<EOF
usage: hack/rehearsal.sh <git-url> <revision>
  builds keel-bin in a throwaway workspace outside the checkout, with the module crates of the
  $product bundle taken from <git-url> at <revision> (a commit, HEAD, a branch or a tag) instead of
  the workspace paths and everything else from the checkout; runs the module crates' tests and
  the bundle's there, proves the binary with hack/prove-binary.sh, and shows that a module crate
  reaching a workspace path is refused.
  KEEL_TEST_DATABASE_URL must point at a Postgres superuser connection, and frontend/dist/shell
  must be built.
  what has to be true of the module crates for the build to succeed:
  - outside their bundle they name only ${reachable[*]}: the throwaway workspace
    patches those to the checkout, and any other crate they reach through a workspace path
    would come from the revision a second time; they may keep naming those through their
    workspace, since a path dependency of a crate taken from a revision resolves into that
    revision's source, which the patch replaces, and names no version the checkout must match;
  - every file they compile, their test targets included, lies inside their own directory;
  - they declare no dev-dependencies, since cargo tests a crate taken from a revision without
    them, and neither their tests nor the bundle's read anything outside the product.
EOF
}

fail() {
  echo "FAILED: $1" >&2
  exit 1
}

cleanup() {
  local code=$?
  trap - EXIT
  if [ -n "$tmp" ] && [ -d "$tmp" ]; then
    if [ -f "$tmp/checkouts" ]; then
      checkouts | comm -13 "$tmp/checkouts" - | while read -r checkout; do
        case "$(basename "$(dirname "$checkout")")" in
          "$(basename "${source_url%.git}")"-* | "$(basename "$refused")"-*) rm -rf "$checkout" ;;
        esac
      done
    fi
    rm -rf "$tmp"
  fi
  if [ -n "$refused" ]; then
    rm -rf "$refused"
  fi
  exit "$code"
}

checkouts() {
  if [ -d "$cargo_home/git/checkouts" ]; then
    find "$cargo_home/git/checkouts" -mindepth 2 -maxdepth 2 -type d | sort
  fi
}

resolve() {
  local source=$1 wanted=$2 refs
  if [[ $wanted =~ ^[0-9a-f]{40}$ ]]; then
    echo "$wanted"
    return 0
  fi
  if ! refs=$(git ls-remote "$source"); then
    echo "could not list the refs of $source" >&2
    return 1
  fi
  awk -v wanted="$wanted" '
    $2 == wanted && exact == "" { exact = $1 }
    $2 == "refs/tags/" wanted "^{}" { peeled = $1 }
    $2 == "refs/tags/" wanted && tag == "" { tag = $1 }
    $2 == "refs/heads/" wanted && branch == "" { branch = $1 }
    END { print (exact != "" ? exact : peeled != "" ? peeled : tag != "" ? tag : branch) }
  ' <<<"$refs"
}

workspace() {
  local dir=$1 source=$2 commit=$3 entry
  mkdir -p "$dir/$binary" "$dir/products/$product" "$dir/frontend/dist"
  if ! python3 - "$root" "$dir" "$source" "$commit" "products/$product" "$binary" "${reachable[@]}" <<'EOF'
import json, os, re, sys

root, workspace, source, commit, bundle, binary = sys.argv[1:7]
reachable = sys.argv[7:]
modules_dir = bundle + "/crates/"
with open(os.path.join(root, "Cargo.toml")) as handle:
    lines = handle.read().splitlines()
section = None
out = []
modules = []
paths = {}
problems = []
for line in lines:
    header = re.match(r"^\[(.+)\]\s*$", line)
    if header:
        section = header.group(1)
    elif section == "workspace" and re.match(r"^members\s*=", line):
        line = "members = " + json.dumps([binary, bundle])
    elif section == "workspace.dependencies":
        dependency = re.match(r'^([A-Za-z0-9_-]+) = \{ path = "([^"]+)"(.*)$', line)
        if dependency:
            name, path, rest = dependency.groups()
            if path.startswith(modules_dir):
                modules.append(name)
                line = f"{name} = {{ git = {json.dumps(source)}, rev = {json.dumps(commit)}{rest}"
            elif path != bundle:
                paths[name] = os.path.join(root, path)
                line = f"{name} = {{ path = {json.dumps(paths[name])}{rest}"
        elif re.search(r"\bpath\s*=", line):
            problems.append(f"the workspace dependency '{line}' names a path this script cannot rewrite")
    out.append(line)
if not modules:
    problems.append(f"the workspace names no module crate under {modules_dir}")
for name in reachable:
    if name not in paths:
        problems.append(f"the workspace names no path for {name}")
if problems:
    for problem in problems:
        print(problem, file=sys.stderr)
    sys.exit(1)
out += ["", f"[patch.{json.dumps(source)}]"]
out += [f"{name} = {{ path = {json.dumps(paths[name])} }}" for name in reachable]
with open(os.path.join(workspace, "Cargo.toml"), "w") as handle:
    handle.write("\n".join(out) + "\n")
EOF
  then
    fail "could not write the manifest of the throwaway workspace"
  fi
  cp "$root/Cargo.lock" "$root/rust-toolchain.toml" "$dir/"
  cp "$root/$binary/Cargo.toml" "$dir/$binary/"
  cp -R "$root/$binary/src" "$dir/$binary/"
  for entry in "$root/products/$product"/*; do
    if [ "$(basename "$entry")" != crates ]; then
      cp -R "$entry" "$dir/products/$product/"
    fi
  done
  cp -R "$root/frontend/dist/shell" "$dir/frontend/dist/"
}

graph() {
  local dir=$1 source=$2 commit=$3
  if ! (cd "$dir" && cargo metadata --format-version 1 >"$dir/metadata.json" 2>"$dir/metadata.log"); then
    sed 's/^/  /' "$dir/metadata.log" >&2
    fail "cargo could not resolve the throwaway workspace"
  fi
  if ! python3 - "$dir/metadata.json" "$source" "${reachable[@]}" >"$dir/crates" <<'EOF'
import json, os, sys

path, source = sys.argv[1:3]
reachable = sys.argv[3:]
with open(path) as handle:
    meta = json.load(handle)
packages = {package["id"]: package for package in meta["packages"]}
modules = set()
for member in meta["workspace_members"]:
    for dependency in packages[member]["dependencies"]:
        if (dependency.get("source") or "").startswith("git+" + source):
            modules.add(dependency["name"])
revision = {
    key: package
    for key, package in packages.items()
    if (package.get("source") or "").startswith("git+" + source + "?")
    or (package.get("source") or "").startswith("git+" + source + "#")
}
edges = {node["id"]: [dep["pkg"] for dep in node["deps"]] for node in meta["resolve"]["nodes"]}
problems = []
if not modules:
    problems.append(f"the throwaway workspace takes no crate from {source}")
for name in sorted(modules - {package["name"] for package in revision.values()}):
    problems.append(f"{name} does not come from {source}")
for key, package in sorted(revision.items(), key=lambda item: item[1]["name"]):
    name = package["name"]
    if name in modules:
        dev = sorted({d["name"] for d in package["dependencies"] if d["kind"] == "dev"})
        if dev:
            problems.append(f"{name} declares the dev-dependencies {', '.join(dev)}; cargo tests a crate taken from a revision without them")
        continue
    by = ", ".join(sorted(packages[other]["name"] for other in revision if key in edges.get(other, [])))
    if name in reachable:
        problems.append(f"{name} {package['version']} comes from the revision: the checkout's {name} does not satisfy what {by} ask for")
    else:
        problems.append(f"{name} comes from the revision a second time, reached by {by} through a path into its workspace; outside the bundle a module crate names only {', '.join(reachable)}")
if problems:
    for problem in problems:
        print(problem, file=sys.stderr)
    sys.exit(1)
for package in sorted(revision.values(), key=lambda package: package["name"]):
    print(package["name"], os.path.dirname(package["manifest_path"]))
EOF
  then
    fail "the module crates of the revision reach outside their bundle"
  fi
  echo "ok: $(awk '{print $1}' "$dir/crates" | tr '\n' ' ')come from $source at $commit"
  echo "ok: nothing else comes from the revision: outside the bundle the module crates name only ${reachable[*]}, which the checkout provides"
}

sources() {
  local dir=$1
  if ! python3 - "$dir/build.json" "$dir/crates" <<'EOF'
import json, os, re, sys

messages, crates = sys.argv[1:3]
dirs = {}
with open(crates) as handle:
    for line in handle:
        name, directory = line.rstrip("\n").split(" ", 1)
        dirs[os.path.realpath(directory)] = name
seen = set()
problems = []
with open(messages) as handle:
    for line in handle:
        if not line.startswith("{"):
            continue
        message = json.loads(line)
        if message.get("reason") != "compiler-artifact":
            continue
        tested = message["profile"]["test"]
        if not tested and "lib" not in message["target"]["kind"]:
            continue
        directory = os.path.realpath(os.path.dirname(message["manifest_path"]))
        if directory not in dirs:
            continue
        depinfo = None
        if tested:
            if message.get("executable"):
                depinfo = os.path.splitext(message["executable"])[0] + ".d"
        else:
            for filename in message["filenames"]:
                found = re.match(r"^lib(.+)\.(rlib|rmeta)$", os.path.basename(filename))
                if found and os.path.basename(os.path.dirname(filename)) == "deps":
                    depinfo = os.path.join(os.path.dirname(filename), found.group(1) + ".d")
                    break
        if depinfo is None or not os.path.isfile(depinfo):
            problems.append(f"cargo left no dep-info for {dirs[directory]}")
            continue
        seen.add(dirs[directory])
        with open(depinfo) as info:
            first = info.readline()
        inputs = first.split(": ", 1)[1] if ": " in first else ""
        for item in re.split(r"(?<!\\)\s+", inputs.strip()):
            if not item:
                continue
            item = item.replace("\\ ", " ")
            path = os.path.realpath(item if os.path.isabs(item) else os.path.join(directory, item))
            if not path.startswith(directory + os.sep):
                problems.append(f"{dirs[directory]} compiles {path}, which lies outside its own directory")
for name in sorted(set(dirs.values()) - seen):
    problems.append(f"the build left no artifact of {name}")
if problems:
    for problem in problems:
        print(problem, file=sys.stderr)
    sys.exit(1)
EOF
  then
    fail "the module crates of the revision compile files outside their own directories"
  fi
  echo "ok: every file the module crates compile, their test targets included, lies inside their own directory"
}

build() {
  local dir=$1
  shift
  if ! (cd "$dir" && cargo "$@" --message-format json-render-diagnostics >>"$dir/build.json"); then
    fail "cargo $* failed in the throwaway workspace"
  fi
}

rehearse() {
  local dir=$1 source=$2 commit=$3 full=$4 name
  local -a selected=()
  workspace "$dir" "$source" "$commit"
  graph "$dir" "$source" "$commit"
  while read -r name _; do
    selected+=(-p "$name")
  done <"$dir/crates"
  : >"$dir/build.json"
  if [ "$full" = true ]; then
    (cd "$dir" && cargo clean -q -p keel-bin -p "keel-$product" "${selected[@]}")
    build "$dir" build -p keel-bin
  else
    build "$dir" build "${selected[@]}"
  fi
  build "$dir" test --no-run "${selected[@]}"
  sources "$dir"
  if [ "$full" = true ]; then
    if ! (cd "$dir" && cargo test -p "keel-$product" "${selected[@]}"); then
      fail "the tests of the module crates or of the bundle failed in the throwaway workspace"
    fi
    echo "ok: the module crates' tests and the bundle's pass against the crates from the revision"
  fi
}

tamper() {
  local what=$1 crate
  rm -rf "$refused"
  git clone -q --no-checkout "$root" "$refused"
  git -C "$refused" checkout -q --detach "$(git -C "$root" rev-parse HEAD)"
  crate=$(find "$refused/products/$product/crates" -mindepth 1 -maxdepth 1 -type d | sort | head -n 1)
  case "$what" in
    path)
      awk '{ print } /^\[dependencies\]$/ { print "keel-types = { path = \"../../../../crates/types\" }" }' \
        "$crate/Cargo.toml" >"$crate/Cargo.toml.new"
      mv "$crate/Cargo.toml.new" "$crate/Cargo.toml"
      ;;
    tests)
      printf '\n[dev-dependencies]\nkeel-testkit = { workspace = true }\n' >>"$crate/Cargo.toml"
      ;;
    file)
      printf '\nconst _: &str = include_str!("../../../../../Cargo.toml");\n' >>"$crate/src/lib.rs"
      ;;
    test-file)
      printf '\n#[cfg(test)]\nconst _: &str = include_str!("../../../../../Cargo.toml");\n' >>"$crate/src/lib.rs"
      ;;
  esac
  git -C "$refused" -c user.name=rehearsal -c user.email=rehearsal@keel.invalid commit -qam "Reach a workspace path"
  git -C "$refused" rev-parse HEAD
}

refuse() {
  local what=$1 label=$2 expected=$3 commit log status
  commit=$(tamper "$what")
  log="$tmp/refused-$what.log"
  set +e
  (
    set -e
    rehearse "$tmp/refused-$what" "file://$refused" "$commit" false
  ) >"$log" 2>&1
  status=$?
  set -e
  if [ "$status" -eq 0 ]; then
    sed 's/^/  /' "$log" >&2
    fail "the rehearsal accepted a module crate that $label"
  fi
  if ! grep -q "$expected" "$log"; then
    sed 's/^/  /' "$log" >&2
    fail "the rehearsal refused a module crate that $label, but not because $expected"
  fi
  echo "ok: a module crate that $label is refused: $(grep -m 1 "$expected" "$log")"
}

if [ -z "$source_url" ] || [ -z "$revision" ]; then
  usage
  exit 2
fi
if [ -z "${KEEL_TEST_DATABASE_URL:-}" ]; then
  usage
  echo "KEEL_TEST_DATABASE_URL is not set" >&2
  exit 2
fi
if [ ! -f "$root/frontend/dist/shell/index.html" ]; then
  usage
  echo "frontend/dist/shell is not built; run pnpm build in frontend/" >&2
  exit 2
fi
for tool in git python3 cargo; do
  if ! command -v "$tool" >/dev/null; then
    echo "$tool is not installed" >&2
    exit 2
  fi
done

target=${CARGO_TARGET_DIR:-target}
case "$target" in
  /*) ;;
  *) target="$root/$target" ;;
esac
target="$target/rehearsal"
mkdir -p "$target"
export CARGO_TARGET_DIR=$target
export CARGO_INCREMENTAL=0
cargo_home=${CARGO_HOME:-$HOME/.cargo}
refused="$target/refused"
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

commit=$(resolve "$source_url" "$revision")
if [ -z "$commit" ]; then
  fail "$source_url has no revision $revision"
fi
tmp=$(mktemp -d "${TMPDIR:-/tmp}/keel-rehearsal.XXXXXX")
tmp=$(cd "$tmp" && pwd -P)
checkouts >"$tmp/checkouts"
case "$tmp" in
  "$root"/*) fail "the throwaway workspace $tmp lies inside the checkout" ;;
esac
echo "rehearsing keel-bin with the $product module crates of $source_url at $commit in $tmp/workspace"

rehearse "$tmp/workspace" "$source_url" "$commit" true
"$root/hack/prove-binary.sh" "$target/debug/keel"
echo "ok: hack/prove-binary.sh passes against the binary built from the revision"

refuse path "depends on keel-types by a path into the workspace" "keel-types comes from the revision a second time"
refuse tests "declares dev-dependencies for its tests" "declares the dev-dependencies keel-testkit"
refuse file "compiles a file outside its own directory" "which lies outside its own directory"
refuse test-file "compiles a file outside its own directory in its tests" "which lies outside its own directory"
echo "the rehearsal passed"
