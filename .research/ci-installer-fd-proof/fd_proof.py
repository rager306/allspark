import errno, json, os, pathlib, shutil, subprocess, tempfile
root = pathlib.Path(__file__).resolve().parent
with tempfile.TemporaryDirectory(prefix="owned-", dir=root) as temp:
    path = pathlib.Path(temp) / "payload"
    writer = open(path, "wb")
    writer.write(b"#!/bin/sh\necho fixture\n"); writer.flush()
    os.chmod(path, 0o755)
    ready_read, ready_write = os.pipe()
    release_read, release_write = os.pipe()
    pid = os.fork()
    if pid == 0:
        os.close(ready_read); os.close(release_write)
        os.write(ready_write, b"R")
        os.read(release_read, 1)
        os._exit(0)
    os.close(ready_write); os.close(release_read)
    assert os.read(ready_read, 1) == b"R"
    writer.close()
    try:
        subprocess.run([str(path)], capture_output=True, check=True)
        busy_errno = None
    except OSError as error:
        busy_errno = error.errno
    assert busy_errno == errno.ETXTBSY, busy_errno
    isolated = pathlib.Path(temp) / "isolated"
    subprocess.run(["cp", str(path), str(isolated)], check=True)
    clean = subprocess.run([str(isolated)], capture_output=True, check=True)
    assert clean.stdout == b"fixture\n"
    os.write(release_write, b"X"); os.waitpid(pid, 0)
    restored = subprocess.run([str(path)], capture_output=True, check=True)
    assert restored.stdout == b"fixture\n"
    print(json.dumps({"inherited_writer_errno":busy_errno,"parent_writer_closed":True,"isolated_copy_exec":True,"after_child_release_exec":True}))
