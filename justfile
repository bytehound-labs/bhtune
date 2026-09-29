check:
    just fmt check
    just lint
    just test
    just deny
    cargo run --locked -p bhtune-server --example gen_openapi
    LEFTHOOK=0 git diff --exit-code -- openapi.json
    cargo run --locked -p bhtune-cli --example gen_docs --features schemars
    LEFTHOOK=0 git diff --exit-code -- docs/reference/ man/ completions/
    cargo build --workspace --locked

fmt mode="fix":
    @case "{{ mode }}" in \
        fix) cargo fmt --all ;; \
        check) cargo fmt --check --all ;; \
        *) echo "Use 'just fmt' or 'just fmt check'." >&2; exit 2 ;; \
    esac

lint:
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

test:
    cargo test --workspace --locked

cov:
    cargo llvm-cov --workspace --locked --lcov --output-path lcov.info

gen:
    cargo run --locked -p bhtune-server --example gen_openapi
    cargo run --locked -p bhtune-cli --example gen_docs --features schemars
    pnpm --filter bhtune-frontend run generate:api

fe:
    pnpm run check:licenses
    pnpm --filter bhtune-frontend run generate:api
    LEFTHOOK=0 git diff --exit-code -- frontend/src/api/schema.d.ts
    pnpm --filter bhtune-frontend run format:check
    pnpm --filter bhtune-frontend run lint
    pnpm --filter bhtune-frontend run test
    pnpm --filter bhtune-frontend run build

e2e:
    cargo build -p bhtune-server --locked
    pnpm --filter bhtune-frontend run build
    cd frontend && npx playwright install --with-deps chromium
    CI=true PLAYWRIGHT_MODE=full PLAYWRIGHT_HTML_OUTPUT_DIR="$(pwd)/frontend/playwright-report/full" pnpm --filter bhtune-frontend exec playwright test --project=full
    CI=true PLAYWRIGHT_MODE=demo PLAYWRIGHT_HTML_OUTPUT_DIR="$(pwd)/frontend/playwright-report/demo" pnpm --filter bhtune-frontend exec playwright test --project=demo

deny:
    cargo deny check
    cargo machete

dev:
    #!/usr/bin/env python3
    import json
    import os
    import shutil
    import signal
    import socket
    import subprocess
    import tempfile
    import time
    from pathlib import Path

    root = Path.cwd()
    for port in (8787, 5173):
        probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            probe.bind(("0.0.0.0", port))
        except OSError as exc:
            raise SystemExit(f"Port {port} must be free before running `just dev`: {exc}")
        finally:
            probe.close()

    node = shutil.which("node")
    vite = root / "frontend" / "node_modules" / "vite" / "bin" / "vite.js"
    if node is None or not vite.is_file():
        raise SystemExit("Install the pnpm workspace dependencies before running `just dev`.")

    subprocess.run(["cargo", "build", "--locked", "-p", "bhtune-server"], check=True)
    metadata = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--locked", "--format-version", "1", "--no-deps"],
            text=True,
        )
    )
    target_dir = Path(metadata["target_directory"])
    build_target = os.environ.get("CARGO_BUILD_TARGET")
    if build_target:
        target_dir /= build_target
    server = target_dir / "debug" / (
        "bhtune-server.exe" if os.name == "nt" else "bhtune-server"
    )
    if not server.is_file():
        raise SystemExit(f"Built bhtune-server binary was not found at {server}.")

    temp_dir = None
    children = []
    stopping = False
    exit_code = 0

    def stop(signum, _frame):
        global stopping, exit_code
        stopping = True
        exit_code = 128 + signum

    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGTERM, stop)

    try:
        temp_dir = Path(tempfile.mkdtemp(prefix="bhtune-just-dev-"))
        log_dir = temp_dir / "logs"
        log_dir.mkdir()
        db_path = temp_dir / "bhtune.db"
        config_path = temp_dir / "bhtune.toml"
        config_path.write_text(
            f"db = {json.dumps(str(db_path))}\n"
            f"[log]\ndir = {json.dumps(str(log_dir))}\n",
            encoding="utf-8",
        )

        server_env = os.environ.copy()
        server_env.update(
            {
                "BHTUNE_DB": str(db_path),
                "BHTUNE_BIND": "0.0.0.0:8787",
                "BHTUNE_SERVER_MODE": "full",
                "BHTUNE_ORIGIN": os.environ.get(
                    "BHTUNE_ORIGIN", "http://localhost:5173"
                ),
            }
        )

        if not stopping:
            server_process = subprocess.Popen(
                [str(server), "--config", str(config_path)],
                cwd=root,
                env=server_env,
            )
            children.append(server_process)
        if not stopping:
            vite_process = subprocess.Popen(
                [node, str(vite), "--host", "0.0.0.0", "--port", "5173", "--strictPort"],
                cwd=root,
            )
            children.append(vite_process)
            print(
                "BHTune server: http://localhost:8787; Vite UI: http://localhost:5173 "
                "(Ctrl+C stops both and removes the temporary database/logs).",
                flush=True,
            )

        while not stopping:
            for child in children:
                child_status = child.poll()
                if child_status is not None:
                    exit_code = child_status if child_status != 0 else 1
                    stopping = True
                    break
            if not stopping:
                time.sleep(0.25)
    except KeyboardInterrupt:
        exit_code = 130
    finally:
        for child in children:
            if child.poll() is None:
                child.terminate()
        deadline = time.monotonic() + 5
        for child in children:
            try:
                child.wait(timeout=max(0.1, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        if temp_dir is not None:
            shutil.rmtree(temp_dir, ignore_errors=True)

    raise SystemExit(exit_code)
