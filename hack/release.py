#!/usr/bin/env python3
from __future__ import annotations

import argparse
import base64
import hashlib
import http.client
import io
import json
import os
import re
import subprocess
import sys
import tarfile
import tempfile
import urllib.error
import urllib.request
from pathlib import Path
from typing import Callable

ROOT = Path(__file__).resolve().parent.parent
TAG_PREFIX = "keel-v"
ARCHIVE_TARGET = "x86_64-linux-musl"
CHANNEL_URL = "https://dl.globalart.hu/keel/"
ASTRUM_URL = "https://astrum.globalart.hu/repository/keel-releases/keel/"
ASTRUM_USER = "keel-release"
KEY_VARIABLE = "KEEL_RELEASE_KEY"
PASSWORD_VARIABLE = "KEEL_RELEASE_ASTRUM_PASSWORD"
CHANNEL_FILE = ROOT / "crates" / "release" / "channel.json"
UNIT_DIR = ROOT / "deploy" / "systemd"
CONFIG_SAMPLE = ROOT / "deploy" / "config-sample.yaml"
DEPLOY_DIR = ROOT / "deploy"
RELEASE_KEY_FILE = ROOT / "crates" / "release" / "keys" / "release.pub"
INSTALLER = "install.sh"
DEPLOY_FILES = {"agent.yaml": 0o644, "uninstall.sh": 0o755}
KEY_LINE = re.compile(r"^RELEASE_KEY='[^']*'$", re.MULTILINE)
KEEL_RELEASE = ["cargo", "run", "-q", "-p", "keel-release", "--"]
IDENTIFIER = r"(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)"
SEMVER = re.compile(
    r"^(?P<major>0|[1-9]\d*)\.(?P<minor>0|[1-9]\d*)\.(?P<patch>0|[1-9]\d*)"
    rf"(?:-(?P<pre>{IDENTIFIER}(?:\.{IDENTIFIER})*))?"
    r"(?:\+(?P<build>[0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$"
)
CHUNK = 1024 * 1024
TIMEOUT = 120


class Failure(Exception):
    pass


def say(text: str) -> None:
    print(f"release: {text}", flush=True)


def semver_key(version: str) -> tuple:
    match = SEMVER.match(version)
    if not match:
        raise Failure(f"{version} is not a semantic version")
    core = tuple(int(match.group(part)) for part in ("major", "minor", "patch"))
    pre = match.group("pre")
    if pre is None:
        return core + ((1,),)
    identifiers = tuple(
        (0, int(identifier), "") if identifier.isdigit() else (1, 0, identifier)
        for identifier in pre.split(".")
    )
    return core + ((0, identifiers),)


def version_of_tag(tag: str) -> str:
    if not tag.startswith(TAG_PREFIX):
        raise Failure(f"the tag {tag} does not start with {TAG_PREFIX}")
    version = tag[len(TAG_PREFIX) :]
    match = SEMVER.match(version)
    if not match:
        raise Failure(f"the tag {tag} does not name a semantic version after {TAG_PREFIX}")
    if match.group("build"):
        raise Failure(f"the tag {tag} carries build metadata, which a release version does not")
    return version


def channel_of(version: str) -> str:
    match = SEMVER.match(version)
    return "beta" if match and match.group("pre") else "stable"


def archive_name(version: str) -> str:
    return f"keel-{version}-{ARCHIVE_TARGET}.tar.zst"


def read_channel_file(path: Path) -> tuple[int, str]:
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError) as error:
        raise Failure(f"could not read {path}: {error}") from error
    if not isinstance(data, dict) or set(data) != {"contract", "min_upgrade_from"}:
        raise Failure(f"{path} must carry exactly contract and min_upgrade_from")
    contract = data["contract"]
    floor = data["min_upgrade_from"]
    if not isinstance(contract, int) or isinstance(contract, bool) or contract < 1:
        raise Failure(f"the contract in {path} must be a positive integer")
    if not isinstance(floor, str) or not SEMVER.match(floor):
        raise Failure(f"min_upgrade_from in {path} must be a semantic version")
    return contract, floor


def git(*args: str) -> str:
    result = subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise Failure(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def tag_notes(tag: str) -> str:
    ref = f"refs/tags/{tag}"
    kind = git("for-each-ref", "--format=%(objecttype)", ref).strip()
    if kind == "":
        raise Failure(f"the tag {tag} is not in this repository")
    if kind != "tag":
        raise Failure(f"the tag {tag} is not annotated; the release notes are its annotation")
    subject = git("for-each-ref", "--format=%(contents:subject)", ref).strip()
    body = git("for-each-ref", "--format=%(contents:body)", ref).strip()
    notes = subject if not body else f"{subject}\n\n{body}"
    if not notes:
        raise Failure(f"the annotation of {tag} is empty")
    return notes + "\n"


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(CHUNK), b""):
            digest.update(chunk)
    return digest.hexdigest()


def units_of(unit_dir: Path) -> list[Path]:
    units = sorted(
        entry
        for entry in unit_dir.iterdir()
        if entry.is_file()
        and entry.name.startswith("keel")
        and entry.suffix in (".service", ".path")
    )
    if not any(unit.name == "keel.service" for unit in units):
        raise Failure(f"{unit_dir} carries no keel.service")
    return units


def embedded_installer(deploy_dir: Path, key_file: Path) -> bytes:
    key = key_file.read_text().splitlines()
    if not key or not key[0].startswith("ed25519 "):
        raise Failure(f"{key_file} does not start with an ed25519 public key line")
    script = (deploy_dir / INSTALLER).read_text()
    if len(KEY_LINE.findall(script)) != 1:
        raise Failure(f"{deploy_dir / INSTALLER} does not carry exactly one RELEASE_KEY line")
    return KEY_LINE.sub(f"RELEASE_KEY='{key[0]}'", script).encode()


def package(
    binary: Path,
    unit_dir: Path,
    config_sample: Path,
    deploy_dir: Path,
    installer: bytes,
    out: Path,
    level: int,
) -> None:
    mtime = int(os.environ.get("SOURCE_DATE_EPOCH", "0"))

    def entry(
        name: str, mode: int, kind: bytes = tarfile.REGTYPE, size: int = 0
    ) -> tarfile.TarInfo:
        info = tarfile.TarInfo(name)
        info.type = kind
        info.mode = mode
        info.size = size
        info.mtime = mtime
        info.uid = info.gid = 0
        info.uname = info.gname = "root"
        return info

    def add_file(tar: tarfile.TarFile, source: Path, name: str, mode: int) -> None:
        with source.open("rb") as handle:
            tar.addfile(entry(name, mode, size=source.stat().st_size), handle)

    def add_bytes(tar: tarfile.TarFile, data: bytes, name: str, mode: int) -> None:
        tar.addfile(entry(name, mode, size=len(data)), io.BytesIO(data))

    for name in DEPLOY_FILES:
        if not (deploy_dir / name).is_file():
            raise Failure(f"{deploy_dir / name} is not a file")
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.USTAR_FORMAT) as tar:
        add_file(tar, binary, "keel", 0o755)
        add_file(tar, config_sample, "config-sample.yaml", 0o644)
        tar.addfile(entry("deploy", 0o755, tarfile.DIRTYPE))
        add_bytes(tar, installer, f"deploy/{INSTALLER}", 0o755)
        for name, mode in sorted(DEPLOY_FILES.items()):
            add_file(tar, deploy_dir / name, f"deploy/{name}", mode)
        tar.addfile(entry("deploy/systemd", 0o755, tarfile.DIRTYPE))
        for unit in units_of(unit_dir):
            add_file(tar, unit, f"deploy/systemd/{unit.name}", 0o644)
    out.parent.mkdir(parents=True, exist_ok=True)
    try:
        subprocess.run(
            ["zstd", f"-{level}", "-q", "-f", "-o", str(out)],
            input=buffer.getvalue(),
            check=True,
            capture_output=True,
        )
    except FileNotFoundError as error:
        raise Failure("zstd is not installed") from error
    except subprocess.CalledProcessError as error:
        raise Failure(f"zstd failed: {error.stderr.decode(errors='replace').strip()}") from error


def payload_of(envelope: bytes) -> dict:
    try:
        parsed = json.loads(envelope)
        payload = json.loads(base64.b64decode(parsed["payload"], validate=True))
    except (ValueError, KeyError, TypeError) as error:
        raise Failure(f"the manifest is not a signed envelope: {error}") from error
    if not isinstance(payload, dict) or "version" not in payload:
        raise Failure("the signed payload is not a release manifest")
    return payload


class Tool:
    def __init__(self, command: list[str]) -> None:
        self.command = command

    def run(self, *args: str) -> str:
        result = subprocess.run(
            [*self.command, *args], cwd=ROOT, capture_output=True, text=True, check=False
        )
        if result.returncode != 0:
            raise Failure(f"keel-release {args[0]} failed: {result.stderr.strip()}")
        return result.stdout

    def sign(
        self,
        key: Path,
        version: str,
        archive: Path,
        floor: str,
        contract: int,
        notes: Path,
        out: Path,
    ) -> None:
        self.run(
            "sign",
            "--key",
            str(key),
            "--version",
            version,
            "--archive",
            str(archive),
            "--min-upgrade-from",
            floor,
            "--contract",
            str(contract),
            "--notes-file",
            str(notes),
            "--out",
            str(out),
        )

    def verify(self, manifest: Path, key: Path | None) -> None:
        args = ["verify", str(manifest)]
        if key is not None:
            args += ["--key", str(key)]
        self.run(*args)


class Directory:
    def __init__(self, root: Path, publish_url: str) -> None:
        self.root = root
        self.publish_url = publish_url

    def describe(self, path: str) -> str:
        return str(self.root / path)

    def exists(self, path: str) -> bool:
        return (self.root / path).is_file()

    def get(self, path: str) -> bytes | None:
        target = self.root / path
        return target.read_bytes() if target.is_file() else None

    def put(self, path: str, data: bytes, content_type: str) -> None:
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        say(f"dry run: {path} -> {target} ({len(data)} bytes, {content_type})")
        say(f"dry run: a publish would PUT {self.publish_url}{path}")

    def stream(self, path: str, sink: Callable[[bytes], None]) -> None:
        with (self.root / path).open("rb") as handle:
            for chunk in iter(lambda: handle.read(CHUNK), b""):
                sink(chunk)


class Http:
    def __init__(self, base: str, user: str | None = None, password: str | None = None) -> None:
        self.base = base if base.endswith("/") else base + "/"
        self.headers: dict[str, str] = {}
        if user is not None and password is not None:
            token = base64.b64encode(f"{user}:{password}".encode()).decode()
            self.headers["Authorization"] = f"Basic {token}"

    def describe(self, path: str) -> str:
        return self.base + path

    def request(
        self,
        method: str,
        path: str,
        data: bytes | None = None,
        content_type: str | None = None,
    ) -> http.client.HTTPResponse:
        headers = dict(self.headers)
        if content_type is not None:
            headers["Content-Type"] = content_type
        request = urllib.request.Request(
            self.base + path, data=data, method=method, headers=headers
        )
        return urllib.request.urlopen(request, timeout=TIMEOUT)

    def exists(self, path: str) -> bool:
        try:
            with self.request("HEAD", path):
                return True
        except urllib.error.HTTPError as error:
            if error.code == 404:
                return False
            raise Failure(f"HEAD {self.base}{path} answered {error.code}") from error
        except urllib.error.URLError as error:
            raise Failure(f"HEAD {self.base}{path} failed: {error.reason}") from error

    def get(self, path: str) -> bytes | None:
        try:
            with self.request("GET", path) as response:
                return response.read()
        except urllib.error.HTTPError as error:
            if error.code == 404:
                return None
            raise Failure(f"GET {self.base}{path} answered {error.code}") from error
        except urllib.error.URLError as error:
            raise Failure(f"GET {self.base}{path} failed: {error.reason}") from error

    def put(self, path: str, data: bytes, content_type: str) -> None:
        try:
            with self.request("PUT", path, data, content_type) as response:
                say(f"PUT {self.base}{path} ({len(data)} bytes) -> {response.status}")
        except urllib.error.HTTPError as error:
            detail = error.read().decode(errors="replace").strip()
            raise Failure(
                f"PUT {self.base}{path} answered {error.code}: {detail[:500]}"
            ) from error
        except urllib.error.URLError as error:
            raise Failure(f"PUT {self.base}{path} failed: {error.reason}") from error

    def stream(self, path: str, sink: Callable[[bytes], None]) -> None:
        try:
            with self.request("GET", path) as response:
                for chunk in iter(lambda: response.read(CHUNK), b""):
                    sink(chunk)
        except urllib.error.HTTPError as error:
            raise Failure(f"GET {self.base}{path} answered {error.code}") from error
        except urllib.error.URLError as error:
            raise Failure(f"GET {self.base}{path} failed: {error.reason}") from error


def reader_for(channel_url: str, repository: Http, probe: str) -> Http:
    channel = Http(channel_url)
    try:
        with channel.request("GET", probe):
            say(f"reading back through {channel.base}")
            return channel
    except urllib.error.HTTPError as error:
        raise Failure(
            f"{channel.base}{probe} answered {error.code}; "
            "the route answers but does not serve the repository"
        ) from error
    except urllib.error.URLError as error:
        say(
            f"{channel.base} does not answer ({error.reason}); "
            f"reading back through {repository.base}"
        )
        return repository


def key_file(explicit: Path | None, work: Path) -> Path:
    if explicit is not None:
        if not explicit.is_file():
            raise Failure(f"the key {explicit} is not a file")
        return explicit
    text = os.environ.get(KEY_VARIABLE, "")
    if not text.strip():
        raise Failure(f"no release key: pass --key or set {KEY_VARIABLE}")
    path = work / "release.key"
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as handle:
        handle.write(text.strip() + "\n")
    return path


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="hack/release.py",
        description=(
            "Publish a release of keel on its channel: package the binary with the unit files, "
            "the policy template and the installer carrying the public release key "
            f"as versions/<ver>/keel-<ver>-{ARCHIVE_TARGET}.tar.zst, write its .sha256, sign the "
            "manifest with keel-release, upload the four objects and install.sh to the raw "
            f"repository and read them back through {CHANNEL_URL} when that name answers. A tag "
            "with a pre-release suffix lands on the beta channel, any other on stable."
        ),
    )
    add = parser.add_argument
    add("--tag", required=True, help=f"the release tag, {TAG_PREFIX}<semver>")
    add("--binary", required=True, type=Path, help="the keel binary built for the target")
    add("--notes-file", type=Path, help="the release notes; default: the tag's annotation")
    add("--key", type=Path, help=f"the secret release key; default: the text in {KEY_VARIABLE}")
    add(
        "--verify-key",
        type=Path,
        help="the public key to verify with; default: the compiled release keys",
    )
    add(
        "--channel-file",
        type=Path,
        default=CHANNEL_FILE,
        help="the contract and min_upgrade_from of the release",
    )
    add(
        "--unit-dir",
        type=Path,
        default=UNIT_DIR,
        help="the unit files the archive carries under deploy/systemd",
    )
    add(
        "--config-sample",
        type=Path,
        default=CONFIG_SAMPLE,
        help="the sample configuration the archive carries",
    )
    add(
        "--deploy-dir",
        type=Path,
        default=DEPLOY_DIR,
        help="the directory of install.sh, uninstall.sh and agent.yaml the archive carries",
    )
    add(
        "--release-key-file",
        type=Path,
        default=RELEASE_KEY_FILE,
        help="the public release key embedded into install.sh",
    )
    add("--keel-release", type=Path, help="a built keel-release binary instead of cargo run")
    add("--zstd-level", type=int, default=19, help="the zstd compression level of the archive")
    add("--dry-run", action="store_true", help="write the layout under --out, upload nothing")
    add("--out", type=Path, help="the directory of the dry run")
    add("--astrum-url", default=ASTRUM_URL, help="the raw repository the objects are uploaded to")
    add(
        "--astrum-user",
        default=ASTRUM_USER,
        help=f"the deploy account; its password is {PASSWORD_VARIABLE}",
    )
    add(
        "--channel-url",
        default=CHANNEL_URL,
        help="the address installations read the channel from",
    )
    add("--replace", action="store_true", help="publish over a version the channel carries")
    args = parser.parse_args(argv)
    if args.dry_run and args.out is None:
        parser.error("--dry-run needs --out <dir>")
    if not args.dry_run and args.out is not None:
        parser.error("--out belongs to --dry-run")
    if args.zstd_level < 1 or args.zstd_level > 19:
        parser.error("--zstd-level is between 1 and 19")
    return args


def store_for(args: argparse.Namespace):
    if args.dry_run:
        return Directory(args.out, args.astrum_url)
    password = os.environ.get(PASSWORD_VARIABLE, "")
    if not password:
        raise Failure(f"{PASSWORD_VARIABLE} is not set")
    return Http(args.astrum_url, args.astrum_user, password)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    version = version_of_tag(args.tag)
    channel = channel_of(version)
    contract, floor = read_channel_file(args.channel_file)
    if semver_key(floor) > semver_key(version):
        raise Failure(
            f"min_upgrade_from {floor} is above the version {version}; "
            "the release would admit nothing"
        )
    if not args.binary.is_file():
        raise Failure(f"{args.binary} is not a file")
    if not args.config_sample.is_file():
        raise Failure(f"{args.config_sample} is not a file")
    if not (args.deploy_dir / INSTALLER).is_file():
        raise Failure(f"{args.deploy_dir / INSTALLER} is not a file")
    if not args.release_key_file.is_file():
        raise Failure(f"{args.release_key_file} is not a file")
    installer = embedded_installer(args.deploy_dir, args.release_key_file)
    notes = args.notes_file.read_text() if args.notes_file is not None else tag_notes(args.tag)
    tool = Tool([str(args.keel_release)] if args.keel_release is not None else KEEL_RELEASE)
    name = archive_name(version)
    paths = {
        "archive": f"versions/{version}/{name}",
        "sha256": f"versions/{version}/{name}.sha256",
        "manifest": f"versions/{version}/manifest.json",
        "head": f"{channel}/manifest.json",
        "installer": INSTALLER,
    }
    say(
        f"{version} on {channel} from the tag {args.tag} "
        f"(contract {contract}, min_upgrade_from {floor})"
    )

    store = store_for(args)
    if store.exists(paths["manifest"]) and not args.replace:
        raise Failure(
            f"{store.describe(paths['manifest'])} exists; "
            f"{version} is published, --replace publishes over it"
        )
    head = store.get(paths["head"])
    previous = payload_of(head)["version"] if head is not None else None
    if previous is not None and semver_key(previous) > semver_key(version):
        raise Failure(
            f"the {channel} head is {previous}; publishing {version} would move it backwards"
        )

    with tempfile.TemporaryDirectory(prefix="keel-release-") as work_name:
        work = Path(work_name)
        key = key_file(args.key, work)
        archive = work / name
        package(
            args.binary,
            args.unit_dir,
            args.config_sample,
            args.deploy_dir,
            installer,
            archive,
            args.zstd_level,
        )
        digest = sha256_of(archive)
        say(f"packaged {name} ({archive.stat().st_size} bytes, sha256 {digest})")
        notes_path = work / "notes.txt"
        notes_path.write_text(notes)
        manifest = work / "manifest.json"
        tool.sign(key, version, archive, floor, contract, notes_path, manifest)
        manifest_bytes = manifest.read_bytes()
        tool.verify(manifest, args.verify_key)
        signed = payload_of(manifest_bytes)
        if signed["version"] != version or signed["sha256"] != digest:
            raise Failure("the signed manifest does not describe the packaged archive")
        lines = len(notes.splitlines())
        say(f"signed the manifest of {version} ({lines} line{'s' if lines != 1 else ''} of notes)")

        sha256_bytes = f"{digest}  {name}\n".encode()
        store.put(paths["archive"], archive.read_bytes(), "application/octet-stream")
        store.put(paths["sha256"], sha256_bytes, "text/plain")
        store.put(paths["manifest"], manifest_bytes, "application/json")
        store.put(paths["head"], manifest_bytes, "application/json")
        store.put(paths["installer"], installer, "text/x-shellscript")

        reader = store if args.dry_run else reader_for(args.channel_url, store, paths["manifest"])
        expectations = {
            "sha256": sha256_bytes,
            "manifest": manifest_bytes,
            "head": manifest_bytes,
            "installer": installer,
        }
        for label, expected in expectations.items():
            if reader.get(paths[label]) != expected:
                raise Failure(
                    f"{reader.describe(paths[label])} read back differs from what was uploaded"
                )
        hasher = hashlib.sha256()
        reader.stream(paths["archive"], hasher.update)
        if hasher.hexdigest() != digest:
            raise Failure(
                f"{reader.describe(paths['archive'])} read back has sha256 "
                f"{hasher.hexdigest()}, not {digest}"
            )
        say(f"read back {len(expectations) + 1} objects")
        was = f" (was {previous})" if previous is not None else ""
        say(f"{paths['head']} names {version}{was}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except Failure as failure:
        print(f"release.py: {failure}", file=sys.stderr)
        sys.exit(1)
