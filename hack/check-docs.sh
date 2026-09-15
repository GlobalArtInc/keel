#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

python3 - <<'EOF'
import os
import re
import subprocess
import sys

sys.stdout.reconfigure(line_buffering=True)
problems = []

M1_NAMES = {
    "Authenticator": "Authenticator",
    "Client": "Client",
    "Authenticated": "Authenticated",
    "SessionChange": "SessionChange",
    "SecretCipher": "SecretCipher",
    "SecondFactorSource": "SecondFactorSource",
    "parent_id_hash": "parent_id_hash",
    "su_id_hash": "su_id_hash",
    "SettingDef": "SettingDef",
    "SettingsSource": "SettingsSource",
    "PreferenceSource": "PreferenceSource",
    "Call::tz": "fn tz(",
    "ConfigSection": "ConfigSection",
    "Flush": "Flush",
}

M2_NAMES = {
    "embed_frontend!": "macro_rules! embed_frontend",
    "Frontend": "pub struct Frontend",
    "ViewSpec": "pub struct ViewSpec",
    "PermissionSource": "pub trait PermissionSource",
    "ManifestSource": "pub trait ManifestSource",
    "ManifestPatch": "pub struct ManifestPatch",
    "Asset": "pub struct Asset",
    "grants": "pub grants:",
    "keel-dev": 'name = "keel-dev"',
    "provideKeelViews": "export function provideKeelViews",
    "KeelViewContext": "export interface KeelViewContext",
    "bootstrapKeel": "export function bootstrapKeel",
    "PHX_LABELS": "PHX_LABELS",
    "PhxDataTableFetch": "PhxDataTableFetch",
    "KeelMockClient": "export class KeelMockClient",
}

CODE_ROOTS = {"crates": ["crates"], "crates/ or frontend/libs/keel": ["crates", "frontend/libs/keel"]}


def found(pattern, roots):
    for root in roots:
        result = subprocess.run(
            ["grep", "-rqF", "--exclude-dir=node_modules", "--", pattern, root],
            check=False,
        )
        if result.returncode == 0:
            return True
    return False


def check_names(milestone, names, roots_label):
    roots = CODE_ROOTS[roots_label]
    for name, code in names.items():
        if not found(name, ["docs/design"]):
            problems.append(f"the {milestone} name {name} is not found under docs/design/")
        if not found(code, roots):
            problems.append(f"the {milestone} name {name} is not found under {roots_label} as {code!r}")
    print(f"{len(names)} {milestone} names looked up under docs/design/ and {roots_label}")


check_names("M1", M1_NAMES, "crates")
check_names("M2", M2_NAMES, "crates/ or frontend/libs/keel")


def section(text, heading):
    start = text.find(f"\n{heading}\n")
    if start < 0:
        problems.append(f"docs/plan.md has no section {heading!r}")
        return ""
    body = text[start + 1 + len(heading) + 1 :]
    following = re.search(r"^## ", body, re.MULTILINE)
    return body[: following.start()] if following else body


def spans(text):
    flat = re.sub(r"\s+", " ", text)
    return re.findall(r"`([^`]+)`", flat)


with open("docs/plan.md") as handle:
    plan = handle.read()


def run_commands(path):
    commands = []
    with open(path) as handle:
        lines = handle.read().splitlines()
    index = 0
    while index < len(lines):
        line = lines[index]
        match = re.match(r"^(\s*)run:\s*(.*)$", line)
        index += 1
        if not match:
            continue
        indent, value = match.groups()
        if value in ("|", ">", "|-", ">-"):
            while index < len(lines) and (not lines[index].strip() or len(lines[index]) - len(lines[index].lstrip()) > len(indent)):
                if lines[index].strip():
                    commands.append(lines[index].strip())
                index += 1
        else:
            commands.append(value.strip())
    return commands


ci = run_commands(".github/workflows/ci.yml")
ci_text = "\n".join(ci)

TEST_NAME = re.compile(r"^[a-z][a-z0-9]*(_[a-z0-9]+){3,}$")
SPEC_NAME = re.compile(r"^frontend/.*\.spec\.ts$")
COMMAND_PREFIXES = ("hack/", "cargo ", "env ", "pnpm ", "node ", "git ")


def check_checklist(milestone, test_roots):
    heading = f"## {milestone} exit checklist"
    checklist = section(plan, heading)
    cited = spans(checklist)

    tests = sorted({span for span in cited if TEST_NAME.match(span)})
    if not tests:
        problems.append(f"the {milestone} exit checklist cites no test name")
    for test in tests:
        result = subprocess.run(
            ["grep", "-rlE", "--include=*.rs", f"fn {test}\\b", *test_roots],
            check=False,
            capture_output=True,
        )
        if result.returncode != 0:
            problems.append(f"the {milestone} exit checklist cites the test {test}, which is not defined under {', '.join(test_roots)}")
    print(f"{len(tests)} test names of the {milestone} exit checklist looked up")

    specs = sorted({span for span in cited if SPEC_NAME.match(span)})
    for spec in specs:
        if not os.path.isfile(spec):
            problems.append(f"the {milestone} exit checklist cites {spec}, which does not exist")
    if specs:
        print(f"{len(specs)} specs of the {milestone} exit checklist looked up")

    commands = [span for span in cited if span.startswith(COMMAND_PREFIXES)]
    for command in commands:
        if command.startswith("hack/"):
            script = command.split()[0]
            if not os.access(script, os.X_OK):
                problems.append(f"the {milestone} exit checklist cites {command}, but {script} is not an executable file under hack/")
        else:
            prefix = command.split("<", 1)[0].rstrip()
            if prefix not in ci_text:
                problems.append(f"the {milestone} exit checklist cites {command}, which is not a run command of .github/workflows/ci.yml")
    print(f"{len(commands)} commands of the {milestone} exit checklist looked up")


check_checklist("M1", ["crates", "products"])
check_checklist("M2", ["crates", "products", "bin"])

with open("README.md") as handle:
    readme = handle.read()
marker = "The checks below are the ones CI runs"
start = readme.find(marker)
if start < 0:
    problems.append("README.md has no check list introduced by 'The checks below are the ones CI runs'")
    listed = []
else:
    body = readme[start:]
    following = re.search(r"^##+ ", body, re.MULTILINE)
    if following:
        body = body[: following.start()]
    listed = []
    for block in re.findall(r"```\n(.*?)```", body, re.DOTALL):
        listed.extend(line.strip() for line in block.splitlines() if line.strip() and not line.startswith("export "))
    if not listed:
        problems.append("the README's check list is empty")

how = spans(section(plan, "## How we work"))
for command in listed:
    if command not in ci:
        problems.append(f"the README lists {command}, which is not a run command of .github/workflows/ci.yml")
    if command not in how:
        problems.append(f"the README lists {command}, which the section 'How we work' of docs/plan.md does not name")
print(f"{len(listed)} commands of the README's check lists compared with ci.yml and 'How we work'")

for problem in problems:
    print(problem, file=sys.stderr)
if problems:
    sys.exit(1)
print("docs checked")
EOF
