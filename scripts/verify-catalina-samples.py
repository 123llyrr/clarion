#!/usr/bin/env python3
"""Verify recovered Catalina sample identities without executing their contents.

The manifest is an explicit local input, not proof of Apple signature trust or
of the completeness of the original installer. Relative paths are interpreted
from the manifest's directory. Only regular-file byte identities are checked.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import tempfile


def file_identity(stat):
    return (stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns)


def verify_manifest(manifest_path):
    manifest_path = Path(manifest_path).resolve()
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if not isinstance(manifest, dict) or manifest.get("version") != 1:
        raise ValueError("manifest must be an object with version 1")
    samples = manifest.get("samples")
    if not isinstance(samples, list) or not samples:
        raise ValueError("manifest must contain a non-empty samples list")

    results = []
    seen = set()
    for sample in samples:
        if not isinstance(sample, dict):
            raise ValueError("each sample must be an object")
        sample_id = sample.get("id")
        if not isinstance(sample_id, str) or not sample_id or sample_id in seen:
            raise ValueError("sample IDs must be non-empty and unique")
        seen.add(sample_id)
        raw_path = sample.get("path")
        expected_hash = sample.get("sha256")
        expected_size = sample.get("size")
        if not isinstance(raw_path, str) or not raw_path:
            raise ValueError(f"{sample_id}: missing file path")
        if not isinstance(expected_hash, str) or not re.fullmatch(
            r"[0-9a-fA-F]{64}", expected_hash
        ):
            raise ValueError(f"{sample_id}: invalid SHA-256")
        if type(expected_size) is not int or expected_size < 0:
            raise ValueError(f"{sample_id}: invalid byte size")
        path = Path(raw_path)
        if not path.is_absolute():
            path = manifest_path.parent / path
        result = {
            "id": sample_id,
            "path": str(path),
            "expected_size": expected_size,
            "expected_sha256": expected_hash.lower(),
        }
        try:
            if not path.is_file():
                raise OSError("sample is missing or is not a regular file")
            digest = hashlib.sha256()
            with path.open("rb") as stream:
                before = os.fstat(stream.fileno())
                if file_identity(path.stat()) != file_identity(before):
                    raise OSError("sample path changed before hashing")
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(chunk)
                after = os.fstat(stream.fileno())
                named_file = path.stat()
            if file_identity(before) != file_identity(after):
                raise OSError("sample changed during hashing")
            if file_identity(named_file) != file_identity(after):
                raise OSError("sample path changed during hashing")
            result.update(size=after.st_size, sha256=digest.hexdigest())
            result["status"] = (
                "verified"
                if after.st_size == expected_size
                and digest.hexdigest() == expected_hash.lower()
                else "mismatch"
            )
        except OSError as error:
            result.update(status="unavailable", error=str(error))
        results.append(result)

    return {
        "manifest": str(manifest_path),
        "verified": all(item["status"] == "verified" for item in results),
        "samples": results,
        "scope": "regular-file size and SHA-256 against the supplied manifest",
        "limitations": [
            "Does not validate the manifest's origin or an Apple certificate chain.",
            "Does not establish installer completeness or runtime behavior.",
        ],
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--output", type=Path, help="also save the JSON result")
    args = parser.parse_args(argv)
    try:
        result = verify_manifest(args.manifest)
    except (OSError, ValueError) as error:
        print(f"Sample verification failed: {error}", file=sys.stderr)
        return 2
    output = json.dumps(result, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        temporary = None
        try:
            inputs = [args.manifest, *[Path(item["path"]) for item in result["samples"]]]
            for source in inputs:
                same_path = args.output.resolve() == source.resolve()
                same_inode = (
                    args.output.exists()
                    and source.exists()
                    and args.output.samefile(source)
                )
                if same_path or same_inode:
                    raise ValueError("Output must not overwrite the manifest or a sample")
            # Replacing a directory entry never truncates the file another hard
            # link references, even if the output changes after the input check.
            with tempfile.NamedTemporaryFile(
                mode="w", encoding="utf-8", dir=args.output.parent,
                prefix=".catalina-verification-", delete=False,
            ) as stream:
                temporary = Path(stream.name)
                stream.write(output)
            os.replace(temporary, args.output)
        except (OSError, ValueError) as error:
            print(f"Cannot save verification: {error}", file=sys.stderr)
            return 2
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)
    print(output, end="")
    return 0 if result["verified"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
