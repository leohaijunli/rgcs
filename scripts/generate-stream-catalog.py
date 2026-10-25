#!/usr/bin/env python3
"""Generate MR-18's catalog from the resolved maggcs-core MAVLink dependency.

Uses Cargo.lock and the dependency's own common.xml/include graph. The checked-
in Rust table keeps desktop builds independent of Python and source XML paths.
Run with --check in CI, or without it after a deliberate MAVLink update.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "crates/core/src/mavlink/standard_stream_catalog.rs"
TYPE_BYTES = {
    "char": 1, "uint8_t": 1, "int8_t": 1,
    "uint16_t": 2, "int16_t": 2,
    "uint32_t": 4, "int32_t": 4, "float": 4,
    "uint64_t": 8, "int64_t": 8, "double": 8,
}
MAX_MESSAGE_ID = 0xFFFFFF
FIELD_TYPE = re.compile(r"^(\w+)(?:\[(\d+)\])?$")


def resolved_mavlink(offline: bool) -> dict:
    cargo = os.environ.get("CARGO", "cargo")
    rustc = os.environ.get("RUSTC", "rustc")
    version = subprocess.run([rustc, "-vV"], check=True, capture_output=True, text=True)
    host = next(line.removeprefix("host: ") for line in version.stdout.splitlines() if line.startswith("host: "))
    command = [cargo, "metadata", "--format-version=1", "--locked", "--filter-platform", host]
    if offline:
        command.append("--offline")
    result = subprocess.run(command, cwd=ROOT, check=True, capture_output=True, text=True)
    metadata = json.loads(result.stdout)
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    core = next(package for package in packages.values() if package["name"] == "maggcs-core")
    dependency = next(dep for dep in nodes[core["id"]]["deps"] if dep["name"] == "mavlink")
    if "common" not in nodes[dependency["pkg"]]["features"]:
        raise ValueError("maggcs-core must compile the common MAVLink dialect")
    return packages[dependency["pkg"]]


def crc_extra(name: str, fields: list[dict]) -> int:
    """MAVLink CRC_EXTRA: message name and size-ordered non-extension fields."""
    crc = 0xFFFF

    def accumulate(data: bytes) -> None:
        nonlocal crc
        for byte in data:
            tmp = byte ^ (crc & 0xFF)
            tmp ^= (tmp << 4) & 0xFF
            crc = (crc >> 8) ^ (tmp << 8) ^ (tmp << 3) ^ (tmp >> 4)

    accumulate((name + " ").encode("ascii"))
    ordered = sorted((field for field in fields if not field["extension"]),
                     key=lambda field: TYPE_BYTES[field["base_type"]], reverse=True)
    for field in ordered:
        accumulate((field["base_type"] + " ").encode("ascii"))
        accumulate((field["name"] + " ").encode("ascii"))
        if field["array_length"] is not None:
            accumulate(bytes([field["array_length"]]))
    return (crc & 0xFF) ^ (crc >> 8)


def definitions(directory: Path) -> tuple[list[dict], list[tuple[str, str]]]:
    messages = {}
    names = set()
    sources = {}

    def visit(filename: str) -> None:
        if filename in sources:
            return
        if Path(filename).name != filename:
            raise ValueError(f"unexpected dialect include: {filename}")
        content = (directory / filename).read_bytes()
        sources[filename] = hashlib.sha256(content).hexdigest()
        root = ET.fromstring(content)
        for include in root.findall("include"):
            visit((include.text or "").strip())
        for message in root.findall("./messages/message"):
            message_id = int(message.attrib["id"])
            name = message.attrib["name"]
            if not 0 <= message_id <= MAX_MESSAGE_ID or message_id in messages or name in names:
                raise ValueError(f"duplicate/invalid MAVLink identity: {message_id} {name}")
            fields = []
            field_names = set()
            extension = False
            for child in message:
                if child.tag == "extensions":
                    extension = True
                if child.tag != "field":
                    continue
                parsed = FIELD_TYPE.fullmatch(child.attrib["type"])
                if parsed is None:
                    raise ValueError(f"unknown field type: {child.attrib['type']}")
                base_type = parsed[1].replace("uint8_t_mavlink_version", "uint8_t")
                length = int(parsed[2]) if parsed[2] is not None else None
                if base_type not in TYPE_BYTES or (length is not None and not 1 <= length <= 255):
                    raise ValueError(f"unsupported wire type: {child.attrib['type']}")
                field_name = child.attrib["name"]
                if field_name in field_names:
                    raise ValueError(f"duplicate field: {name}.{field_name}")
                field_names.add(field_name)
                fields.append({
                    "name": field_name, "base_type": base_type,
                    "wire_type": base_type + (f"[{length}]" if length is not None else ""),
                    "array_length": length, "units": child.get("units") or None,
                    "extension": extension,
                })
            messages[message_id] = {"id": message_id, "name": name, "fields": fields,
                                    "extra_crc": crc_extra(name, fields)}
            names.add(name)

    visit("common.xml")
    return [messages[key] for key in sorted(messages)], sorted(sources.items())


def render(package: dict, messages: list[dict], sources: list[tuple[str, str]]) -> str:
    def quoted(value: str) -> str:
        return json.dumps(value, ensure_ascii=False)

    lines = [
        "// @generated by scripts/generate-stream-catalog.py; do not edit.",
        f"// Source: resolved mavlink {package['version']}, common dialect.",
        *[f"// SHA256 {filename}: {digest}" for filename, digest in sources],
        "", "use super::{FieldSpec, StreamSpec};", "", "#[rustfmt::skip]",
        "pub(super) const STREAMS: &[StreamSpec] = &[",
    ]
    for message in messages:
        lines.append(f"    StreamSpec {{ id: {message['id']}, name: {quoted(message['name'])}, extra_crc: {message['extra_crc']}, fields: &[")
        for field in message["fields"]:
            units = f"Some({quoted(field['units'])})" if field["units"] else "None"
            lines.append(f"        FieldSpec {{ name: {quoted(field['name'])}, wire_type: {quoted(field['wire_type'])}, units: {units} }},")
        lines.append("    ] },")
    lines.extend(["];"])
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail if the committed table differs")
    parser.add_argument("--offline", action="store_true", help="use only Cargo's cached dependencies")
    args = parser.parse_args()
    try:
        package = resolved_mavlink(args.offline)
        directory = Path(package["manifest_path"]).parent / "mavlink/message_definitions/v1.0"
        messages, sources = definitions(directory)
        text = render(package, messages, sources)
        if args.check:
            if not OUTPUT.is_file() or OUTPUT.read_text(encoding="utf-8") != text:
                print("Stream catalog is stale; run python3 scripts/generate-stream-catalog.py", file=sys.stderr)
                return 1
        else:
            OUTPUT.write_text(text, encoding="utf-8")
        fields = sum(len(message["fields"]) for message in messages)
        print(f"Stream catalog {'checked' if args.check else 'generated'}: {len(messages)} messages, {fields} fields, mavlink {package['version']}")
        return 0
    except subprocess.CalledProcessError as error:
        print(error.stderr or str(error), file=sys.stderr)
        return 1
    except (OSError, ValueError, KeyError, StopIteration, ET.ParseError) as error:
        print(f"Cannot generate stream catalog: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
