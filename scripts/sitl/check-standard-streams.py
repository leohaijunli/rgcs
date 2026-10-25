#!/usr/bin/env python3
"""Run production stream-service E2E against an isolated, prebuilt PX4 SIH instance.

Only the new instance is terminated. Its parameters, logs and evidence live in
the requested artifacts directory. No serial device, motor command or Docker
permission is involved. Requires the usual app test build dependencies.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import tempfile
import time


def udp_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--px4-build", type=Path, default=Path.home() / "PX4-Autopilot-v1.17/build/px4_sitl_default")
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--instance", type=int, default=42)
    args = parser.parse_args()
    if not 1 <= args.instance <= 200:
        parser.error("use an unused nonzero PX4 instance in 1..200")
    root = Path(__file__).resolve().parents[2]
    build = args.px4_build.resolve()
    binary = build / "bin/px4"
    if not binary.is_file() or not (build / "etc/init.d-posix/rcS").is_file():
        parser.error("--px4-build must contain bin/px4 and etc/init.d-posix/rcS")
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    artifact_root = args.artifacts.resolve()
    artifact_root.mkdir(parents=True, exist_ok=True)
    artifacts = Path(tempfile.mkdtemp(prefix="run-", dir=artifact_root))
    work = artifacts / "rootfs"
    work.mkdir()
    fc_port, gcs_port = udp_port(), udp_port()
    while gcs_port == fc_port:
        gcs_port = udp_port()
    env = os.environ.copy()
    env.update({
        "PATH": os.pathsep.join([str(root / "scripts/sitl/standard-streams"), str(build / "bin"), str(Path(cargo).parent), env.get("PATH", "")]),
        "PX4_SIM_MODEL": "sihsim_quadx", "PX4_PARAM_SDLOG_MODE": "-1",
        "MAGGCS_SITL_STREAM_TEST": "1", "MAGGCS_SITL_INSTANCE": str(args.instance),
        "MAGGCS_SITL_FC_PORT": str(fc_port), "MAGGCS_SITL_GCS_PORT": str(gcs_port),
        "MAGGCS_SITL_ENDPOINT": f"udpin:127.0.0.1:{gcs_port}",
        "MAGGCS_SITL_SYSTEM_ID": str(args.instance + 1),
        "MAGGCS_SITL_MAVLINK_BIN": str(build / "bin/px4-mavlink"),
        "MAGGCS_STREAM_REPORT": str(artifacts / "standard-streams.json"),
    })
    context = {"px4_build": str(build), "instance": args.instance, "fc_port": fc_port, "gcs_port": gcs_port, "airframe": "sihsim_quadx", "rootfs": str(work)}
    source = build.parent.parent
    for key, arguments in [("source_version", ["describe", "--tags", "--always", "--dirty"]), ("source_commit", ["rev-parse", "HEAD"])]:
        result = subprocess.run(["git", *arguments], cwd=source, text=True, capture_output=True, check=False)
        context[key] = result.stdout.strip()
    log_path = artifacts / "px4.log"
    with log_path.open("w") as log:
        process = subprocess.Popen([str(binary), "-d", "-i", str(args.instance), "-w", str(work), str(build / "etc")], cwd=work, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            # Wait for this instance's custom link, not another PX4 daemon.
            ready = False
            for _ in range(200):
                if process.poll() is not None:
                    raise RuntimeError(f"isolated PX4 exited; inspect {log_path}")
                startup = log_path.read_text(errors="replace")
                if "Startup script returned successfully" in startup and f"on udp port {fc_port} remote port {gcs_port}" in startup:
                    context["startup_log"] = startup
                    ready = True
                    break
                time.sleep(0.1)
            if not ready:
                raise RuntimeError(f"isolated MAVLink link never became ready; inspect {log_path}")
            offset = log_path.stat().st_size
            version = subprocess.run([str(build / "bin/px4-ver"), "--instance", str(args.instance), "all"], env=env, text=True, capture_output=True, timeout=3, check=False)
            # Some PX4 builds print client-command output to the daemon log.
            context["binary_version"] = version.stdout + log_path.read_bytes()[offset:].decode(errors="replace")
            (artifacts / "context.json").write_text(json.dumps(context, indent=2) + "\n")
            print(f"PX4 {context['source_version']} · isolated instance {args.instance} · evidence {artifacts}", flush=True)
            result = subprocess.run([cargo, "test", "-p", "maggcs-app", "--lib", "standard_streams_sitl_end_to_end", "--", "--ignored", "--nocapture"], cwd=root, env=env, timeout=180, check=False)
            return result.returncode
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGINT)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)


if __name__ == "__main__":
    raise SystemExit(main())
