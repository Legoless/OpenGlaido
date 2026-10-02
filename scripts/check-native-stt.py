#!/usr/bin/env python3
"""Real-model smoke test; explicitly provide every GGUF to test (missing files fail).

python3 scripts/check-native-stt.py --helper src-tauri/binaries/openglaido-stt-aarch64-apple-darwin \
    --wav /path/to/whisper.cpp/samples/jfk.wav /path/to/models/*.gguf
Uses public JFK test audio, never records the microphone or sends audio over a network.
"""
import argparse
import array
import concurrent.futures
import json
import pathlib
import re
import struct
import subprocess
import sys
import tempfile
import time
import wave


def read_exact(pipe, size):
    result = bytearray()
    while len(result) < size:
        data = pipe.read(size - len(result))
        if not data:
            raise RuntimeError("Speech helper exited before completing its response")
        result.extend(data)
    return result


def response(pipe):
    status, length = struct.unpack("<II", read_exact(pipe, 8))
    if status not in (0, 1) or length > 4 * 1024 * 1024:
        raise RuntimeError("Invalid response framing")
    text = read_exact(pipe, length).decode("utf-8")
    if status:
        raise RuntimeError(text)
    return text


def words(text):
    return re.findall(r"[a-z]+", text.lower())


def check(helper, model, pcm):
    language = b"en" if any(name in model.name.lower() for name in ("cohere", "qwen")) else b""
    with tempfile.TemporaryFile() as log, concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
        child = subprocess.Popen([str(helper), str(model)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log)
        try:
            started = time.monotonic()
            assert pool.submit(response, child.stdout).result(timeout=180) == "", "Invalid ready response"
            load_seconds = time.monotonic() - started
            outputs = []
            for name, audio in [("first", pcm), ("warm", pcm), ("long", pcm * 4)]:
                def request():
                    child.stdin.write(struct.pack("<II", len(audio) // 4, len(language)) + language + audio)
                    child.stdin.flush()
                    return response(child.stdout)
                started = time.monotonic()
                text = pool.submit(request).result(timeout=240)
                tokens = words(text)
                assert {"country", "ask", "you"}.issubset(tokens), f"{name}: unexpected transcript {text!r}"
                if name == "long":
                    assert tokens.count("country") >= 6, f"Long recording was truncated: {text!r}"
                outputs.append({"run": name, "seconds": round(time.monotonic() - started, 2), "text": text})
            assert words(outputs[0]["text"]) == words(outputs[1]["text"]), "Repeated request changed after warming the model"
            child.stdin.close()
            assert child.wait(timeout=15) == 0, "Helper did not exit cleanly on EOF"
            print(json.dumps({"model": model.name, "load_seconds": round(load_seconds, 2), "outputs": outputs}), flush=True)
        except BaseException:
            log.seek(0)
            print(log.read().decode("utf-8", errors="replace")[-5000:], file=sys.stderr)
            raise
        finally:
            if child.poll() is None:
                child.kill()
            child.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helper", type=pathlib.Path, required=True)
    parser.add_argument("--wav", type=pathlib.Path, required=True)
    parser.add_argument("models", type=pathlib.Path, nargs="+")
    args = parser.parse_args()
    with wave.open(str(args.wav), "rb") as audio:
        assert (audio.getframerate(), audio.getnchannels(), audio.getsampwidth()) == (16000, 1, 2), "Expected mono 16 kHz int16 JFK audio"
        samples = array.array("h", audio.readframes(audio.getnframes()))
        if sys.byteorder != "little":
            samples.byteswap()
        pcm = b"".join(struct.pack("<f", sample / 32768) for sample in samples)
    for model in args.models:
        assert model.is_file(), f"Missing model: {model}"
        check(args.helper.resolve(), model.resolve(), pcm)


if __name__ == "__main__":
    main()
