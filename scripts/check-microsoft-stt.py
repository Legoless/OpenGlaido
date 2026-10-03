#!/usr/bin/env python3
"""Real, offline Microsoft model checks using an explicitly supplied public/synthetic WAV.

Example (public whisper.cpp JFK sample, never microphone capture):
  python3 scripts/check-microsoft-stt.py --wav /path/to/jfk.wav \
      --expect country --expect ask --expect you vibevoice-bitnet vibevoice-asr

Each requested model must be fully downloaded. Checks are sequential, deadlines are bounded,
and JSON results include cold load, warm requests, and live append/finalization timings.
Use --python /path/to/python --source-helper native-microsoft/helper.py for an unfrozen runtime.
No downloads, credentials, cloud calls, or app configuration changes are performed.
"""
from __future__ import annotations

import argparse
import array
from collections import Counter
import json
import os
from pathlib import Path
import re
import selectors
import struct
import subprocess
import sys
import tempfile
import time
import wave


ROOT = Path(__file__).resolve().parent.parent
FAMILIES = {
    "vibevoice-asr": "vibevoice-asr",
    "vibevoice-asr-streaming-7b": "vibevoice-asr-streaming",
    "phi-4-multimodal": "phi4-multimodal",
}
MODELS = ["vibevoice-bitnet", *FAMILIES]
MAX_REPLY = 4 * 1024 * 1024


def read_wav(path):
    with wave.open(str(path), "rb") as audio:
        if (audio.getframerate(), audio.getnchannels(), audio.getsampwidth(), audio.getcomptype()) != (16000, 1, 2, "NONE"):
            raise ValueError("Expected uncompressed mono 16 kHz int16 WAV")
        samples = array.array("h", audio.readframes(audio.getnframes()))
    if sys.byteorder != "little":
        samples.byteswap()
    if not samples or len(samples) > 16000 * 30 * 60:
        raise ValueError("Provide between one sample and 30 minutes of public/synthetic audio")
    floats = array.array("f", (sample / 32768 for sample in samples))
    if sys.byteorder != "little":
        floats.byteswap()
    return floats.tobytes()


def words(text):
    return re.findall(r"\w+", text.casefold())


def validate(text, expected):
    if not text.strip():
        raise AssertionError("Model returned an empty transcript for the supplied speech")
    needed, actual = Counter(words(" ".join(expected))), Counter(words(text))
    missing = needed - actual
    if missing:
        raise AssertionError(f"Unexpected transcript; missing {dict(missing)}: {text!r}")


def ready_files(args, model):
    manifest = json.loads(args.manifest.read_text())
    if model not in manifest:
        raise ValueError(f"Missing artifact manifest for {model}")
    files = manifest[model]
    deadline = time.monotonic() + args.wait_ready
    while True:
        missing = [entry["file"] for entry in files if not (args.models_dir / entry["file"]).is_file()
                   or (args.models_dir / entry["file"]).stat().st_size != entry["size_bytes"]]
        if not missing:
            return [(args.models_dir / entry["file"]).resolve() for entry in files]
        if time.monotonic() >= deadline:
            raise FileNotFoundError(f"{model} is incomplete: {', '.join(missing)}")
        time.sleep(min(2, max(0, deadline - time.monotonic())))


class Helper:
    def __init__(self, command, load_deadline):
        self.log = tempfile.TemporaryFile()
        self.child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.child.stdout, selectors.EVENT_READ)
        os.set_blocking(self.child.stdin.fileno(), False)
        os.set_blocking(self.child.stdout.fileno(), False)
        started = time.monotonic()
        try:
            if self.reply(started + load_deadline) != "":
                raise AssertionError("Unexpected helper readiness response")
        except BaseException as error:
            diagnostics = self.stderr()
            self.close()
            self.log.close()
            raise RuntimeError(f"{error}\n{diagnostics}") from error
        self.load_seconds = round(time.monotonic() - started, 2)

    def exact(self, count, deadline):
        result = bytearray()
        while len(result) < count:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not self.selector.select(remaining):
                raise TimeoutError("Microsoft helper response deadline exceeded")
            part = os.read(self.child.stdout.fileno(), count - len(result))
            if not part:
                raise RuntimeError(f"Microsoft helper exited before completing a reply ({self.child.poll()})")
            result.extend(part)
        return result

    def reply(self, deadline):
        status, count = struct.unpack("<II", self.exact(8, deadline))
        if status not in (0, 1) or count > MAX_REPLY:
            raise ValueError("Invalid Microsoft helper reply framing")
        text = self.exact(count, deadline).decode("utf-8")
        if status:
            raise RuntimeError(text)
        return text

    def write(self, data, deadline):
        view = memoryview(data)
        with selectors.DefaultSelector() as writer:
            writer.register(self.child.stdin, selectors.EVENT_WRITE)
            while view:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not writer.select(remaining):
                    raise TimeoutError("Microsoft helper request deadline exceeded")
                try:
                    count = os.write(self.child.stdin.fileno(), view)
                except BlockingIOError:
                    continue
                view = view[count:]

    def request(self, operation, pcm, timeout, language="en", hints=("country",)):
        language = language.encode()
        hints = json.dumps(list(hints), ensure_ascii=False).encode()
        header = struct.pack("<4I", operation, len(pcm) // 4, len(language), len(hints))
        deadline = time.monotonic() + timeout
        self.write(header + language + hints + pcm, deadline)
        return self.reply(deadline)

    def stderr(self):
        self.log.seek(0, os.SEEK_END)
        size = self.log.tell()
        self.log.seek(max(0, size - 6000))
        return self.log.read().decode("utf-8", errors="replace")

    def close(self, clean=False):
        if self.child.poll() is None:
            self.child.stdin.close()
            try:
                self.child.wait(timeout=10 if clean else 0.1)
            except subprocess.TimeoutExpired:
                self.child.kill()
                self.child.wait(timeout=10)
                if clean:
                    raise AssertionError("Microsoft helper failed to exit promptly on EOF")
        self.selector.close()
        if clean and self.child.returncode:
            raise AssertionError(f"Microsoft helper exited with status {self.child.returncode}")


def run(args, model, pcm):
    helper = None
    started = time.monotonic()
    try:
        files = ready_files(args, model)
        if model == "vibevoice-bitnet":
            lm = next(path for path in files if "-lm-" in path.name)
            vae = next(path for path in files if "-vae-" in path.name)
            command = [str(args.native_helper.resolve()), "--lm-model", str(lm), "--vae-model", str(vae),
                       "--greedy", "--no-token-stream", "-c", "32768", "--max-tokens", "16384"]
        else:
            # Keep the venv executable path: resolving its symlink would bypass pyvenv.cfg.
            command = ([str(args.python.absolute()), str(args.source_helper.resolve())] if args.python
                       else [str(args.runtime.resolve())]) + [FAMILIES[model], str(files[0].parent)]
        helper = Helper(command, args.load_deadline)
        outputs = []
        for label in ("first", "warm"):
            request_started = time.monotonic()
            run_deadline = request_started + args.deadline
            def remaining():
                seconds = run_deadline - time.monotonic()
                if seconds <= 0:
                    raise TimeoutError("Microsoft transcription session deadline exceeded")
                return seconds
            partials = []
            if model == "vibevoice-bitnet":
                deadline = request_started + args.deadline
                helper.write(b"CONTEXT:country\n", deadline)
                if helper.reply(deadline) != "":
                    raise AssertionError("Invalid dictionary context acknowledgment")
                helper.write(str(args.wav.resolve()).encode() + b"\n", deadline)
                text = helper.reply(deadline)
            elif model == "vibevoice-asr-streaming-7b":
                if helper.request(1, b"", remaining()) != "":
                    raise AssertionError("Live reset returned text from an earlier recording")
                # Send 100 ms live chunks before finalization. Deliberately leave 500 ms for the
                # final packet; this exercises the retained microphone tail rather than EOF.
                tail = min(len(pcm), 16000 * 4 // 2)
                chunk = 16000 * 4 // 10
                last = len(pcm) - tail
                for offset in range(0, last, chunk):
                    append_started = time.monotonic()
                    partial = helper.request(2, pcm[offset:min(offset + chunk, last)], remaining())
                    if partial and (not partials or partial != partials[-1]["text"]):
                        partials.append({"audio_seconds": round(min(offset + chunk, last) / 64000, 3),
                                         "seconds": round(time.monotonic() - append_started, 3), "text": partial})
                text = helper.request(3, pcm[last:], remaining())
                if len(pcm) / 64000 >= 8 and not partials:
                    raise AssertionError("Live model produced no incremental transcript before finalization")
            else:
                text = helper.request(0, pcm, remaining())
            validate(text, args.expect)
            outputs.append({"run": label, "seconds": round(time.monotonic() - request_started, 2),
                            "text": text, **({"partials": partials, "final_tail_seconds": tail / 64000}
                                            if model == "vibevoice-asr-streaming-7b" else {})})
        if words(outputs[0]["text"]) != words(outputs[1]["text"]):
            raise AssertionError("The warmed model changed its deterministic transcript or retained an earlier session")
        helper.close(clean=True)
        result = {"model": model, "status": "passed", "load_seconds": helper.load_seconds,
                  "total_seconds": round(time.monotonic() - started, 2), "outputs": outputs}
        print(json.dumps(result, ensure_ascii=False), flush=True)
        return True
    except Exception as error:
        details = helper.stderr() if helper is not None else ""
        print(json.dumps({"model": model, "status": "failed", "seconds": round(time.monotonic() - started, 2),
                          "error": str(error), "diagnostics": details}, ensure_ascii=False), flush=True)
        return False
    finally:
        if helper is not None:
            helper.close()
            helper.log.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--wav", type=Path, required=True)
    parser.add_argument("--models-dir", type=Path, default=Path.home() / "Library/Application Support/com.openglaido.app/models")
    parser.add_argument("--manifest", type=Path, default=ROOT / "src-tauri/src/models/microsoft-files.json")
    parser.add_argument("--native-helper", type=Path, default=ROOT / "src-tauri/binaries/openglaido-vibe-aarch64-apple-darwin")
    parser.add_argument("--runtime", type=Path, default=ROOT / "src-tauri/microsoft-runtime/openglaido-microsoft")
    parser.add_argument("--python", type=Path)
    parser.add_argument("--source-helper", type=Path, default=ROOT / "native-microsoft/helper.py")
    parser.add_argument("--expect", action="append", default=[], help="Required word/phrase; repeat to require multiple occurrences")
    parser.add_argument("--deadline", type=float, default=600)
    parser.add_argument("--load-deadline", type=float, default=600)
    parser.add_argument("--wait-ready", type=float, default=0, help="Bounded seconds to wait for complete local model artifacts")
    parser.add_argument("models", nargs="*", metavar="MODEL", help="Model IDs (default: all four)")
    args = parser.parse_args()
    if min(args.deadline, args.load_deadline) <= 0 or args.wait_ready < 0:
        parser.error("Deadlines must be positive, and wait-ready cannot be negative")
    if set(args.models) - set(MODELS):
        parser.error(f"Choose model IDs from: {', '.join(MODELS)}")
    pcm = read_wav(args.wav)
    passed = True
    for model in args.models or MODELS:
        passed = run(args, model, pcm) and passed
    raise SystemExit(0 if passed else 1)


if __name__ == "__main__":
    main()
