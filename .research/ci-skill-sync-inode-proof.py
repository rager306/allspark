from pathlib import Path
import errno
import os
import subprocess
import tempfile

root = Path(__file__).resolve().parent
script = b"#!/bin/sh\nprintf 'fixture-ready'\nexit 7\n"
with tempfile.TemporaryDirectory(prefix="skill-sync-inode-", dir=root) as work:
    work = Path(work)
    executable = work / "in-place"
    executable.write_bytes(script)
    executable.chmod(0o755)
    with executable.open("r+b"):
        try:
            subprocess.run([str(executable)], check=False, capture_output=True)
        except OSError as error:
            assert error.errno == errno.ETXTBSY, error
            print("in-place held executable writer: ETXTBSY")
        else:
            raise AssertionError("expected ETXTBSY for held executable writer")
    source = work / "uv-script"
    destination = work / "uv"
    source.write_bytes(script)
    with source.open("r+b"):
        subprocess.run(["cp", str(source), str(destination)], check=True)
        destination.chmod(0o755)
        result = subprocess.run([str(destination)], check=False, capture_output=True)
        assert (result.returncode, result.stdout) == (7, b"fixture-ready"), result
        print("held non-executable source writer + waited cp: executable status7/stdout exact")
