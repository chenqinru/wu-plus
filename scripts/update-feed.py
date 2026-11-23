#!/usr/bin/env python3
import argparse
import base64
import hashlib
import json
import os
import plistlib
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from xml.etree import ElementTree as ET

SPARKLE = "http://www.andymatuschak.org/xml-namespaces/sparkle"
ET.register_namespace("sparkle", SPARKLE)


def sparkle(name):
    return f"{{{SPARKLE}}}{name}"


def checksum(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def app_info(app):
    with (app / "Contents/Info.plist").open("rb") as stream:
        return plistlib.load(stream)


def verify_archive_signature(archive, public_key, signature):
    key = base64.b64decode(public_key.strip(), validate=True)
    if len(key) != 32:
        raise ValueError("Invalid Ed25519 public key")
    with tempfile.TemporaryDirectory() as temporary:
        key_path = Path(temporary) / "public.der"
        signature_path = Path(temporary) / "archive.sig"
        key_path.write_bytes(bytes.fromhex("302a300506032b6570032100") + key)
        signature_path.write_bytes(base64.b64decode(signature, validate=True))
        subprocess.run([
            os.environ.get("OPENSSL", "openssl"), "pkeyutl", "-verify", "-rawin", "-pubin",
            "-keyform", "DER", "-inkey", key_path, "-in", archive, "-sigfile", signature_path,
        ], check=True, stdout=sys.stderr)


def create(archive, app, url, signature, output):
    info = app_info(app)
    if len(base64.b64decode(signature, validate=True)) != 64:
        raise ValueError("Invalid Sparkle enclosure signature")
    root = ET.Element("rss", version="2.0")
    channel = ET.SubElement(root, "channel")
    ET.SubElement(channel, "title").text = "Wu Plus"
    item = ET.SubElement(channel, "item")
    ET.SubElement(item, "title").text = f"Wu Plus {info['CFBundleShortVersionString']}"
    ET.SubElement(item, sparkle("version")).text = info["CFBundleVersion"]
    ET.SubElement(item, sparkle("shortVersionString")).text = info["CFBundleShortVersionString"]
    ET.SubElement(item, sparkle("minimumSystemVersion")).text = info["LSMinimumSystemVersion"]
    ET.SubElement(item, "enclosure", {
        "url": url,
        "length": str(archive.stat().st_size),
        "type": "application/octet-stream",
        sparkle("edSignature"): signature,
    })
    ET.indent(root, space="  ")
    ET.ElementTree(root).write(output, encoding="utf-8", xml_declaration=True)


def check(directory, app, repository_url, expected_sha, expected_key, project_sha):
    manifest = json.loads((directory / "candidate.json").read_text())
    archive = directory / "Wu-Plus-arm64.zip"
    feed = directory / "appcast.xml"
    if checksum(archive) != expected_sha or manifest["archive_sha256"] != expected_sha:
        raise ValueError("Approved archive checksum does not match the candidate")
    if not manifest["notarized"] or checksum(feed) != manifest["appcast_sha256"]:
        raise ValueError("Candidate must include its original appcast and notarized archive")
    if manifest["dirty"] or manifest["project_sha"] != project_sha:
        raise ValueError("Candidate source does not match the build run")
    info = app_info(app)
    build = manifest["build"]
    if not re.fullmatch(r"[1-9][0-9]{0,3}\.[1-9][0-9]?", build):
        raise ValueError("Invalid candidate build")
    if (
        info["CFBundleIdentifier"] != "io.github.chenqinru.wu-plus"
        or info["CFBundleVersion"] != build
        or f"v{info['CFBundleShortVersionString']}" != manifest["upstream"]["tag"]
        or info.get("SUPublicEDKey") != expected_key.strip()
        or not info.get("SURequireSignedFeed")
        or info.get("SUFeedURL") != f"{repository_url}/releases/latest/download/appcast.xml"
    ):
        raise ValueError("Candidate bundle metadata does not match the production update channel")
    items = ET.parse(feed).findall("channel/item")
    if len(items) != 1:
        raise ValueError("Candidate appcast must describe one update")
    item = items[0]
    enclosure = item.find("enclosure")
    if (
        item.findtext(sparkle("version")) != build
        or item.findtext(sparkle("shortVersionString")) != info["CFBundleShortVersionString"]
        or enclosure is None
        or enclosure.get("url") != f"{repository_url}/releases/download/wu-plus-{build}/{archive.name}"
        or enclosure.get("length") != str(archive.stat().st_size)
        or len(base64.b64decode(enclosure.get(sparkle("edSignature"), ""), validate=True)) != 64
    ):
        raise ValueError("Appcast does not describe the approved archive")
    verify_archive_signature(archive, expected_key, enclosure.get(sparkle("edSignature")))
    print(build)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="mode", required=True)
    writer = sub.add_parser("create")
    writer.add_argument("archive", type=Path)
    writer.add_argument("app", type=Path)
    writer.add_argument("url")
    writer.add_argument("signature")
    writer.add_argument("output", type=Path)
    verifier = sub.add_parser("check")
    verifier.add_argument("directory", type=Path)
    verifier.add_argument("app", type=Path)
    verifier.add_argument("repository_url")
    verifier.add_argument("expected_sha")
    verifier.add_argument("expected_key")
    verifier.add_argument("project_sha")
    arguments = vars(parser.parse_args())
    mode = arguments.pop("mode")
    (create if mode == "create" else check)(**arguments)
