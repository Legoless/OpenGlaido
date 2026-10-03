#!/usr/bin/env python3
"""Offline Microsoft speech helper. stdout is a bounded binary protocol, never model logs.

Start: openglaido-microsoft {vibevoice-asr,vibevoice-asr-streaming,phi4-multimodal} MODEL_DIR
Request: little-endian u32 operation, sample count, language bytes, hints bytes;
         language UTF-8, hints JSON string array, mono 16 kHz float32 PCM.
Operations: 0 completed recording, 1 reset live, 2 append live, 3 finish live.
Reply: little-endian u32 status (0 success, 1 error), UTF-8 byte count, text.
"""
from __future__ import annotations

import array
import io
import json
import math
import os
from pathlib import Path
import re
import struct
import sys

# Model modules live inside the signed app's resource folder.
sys.dont_write_bytecode = True

RATE = 16_000
MAX_SAMPLES = RATE * 30 * 60
MAX_METADATA = 64 * 1024
MAX_RESPONSE = 4 * 1024 * 1024
FAMILIES = {"vibevoice-asr", "vibevoice-asr-streaming", "phi4-multimodal"}


def exact(reader, size):
    data = reader.read(size)
    if len(data) != size:
        raise ValueError("Incomplete local speech request")
    return data


def request(reader):
    header = reader.read(16)
    if not header:
        return None
    if len(header) != 16:
        raise ValueError("Incomplete local speech header")
    operation, count, language_size, hints_size = struct.unpack("<4I", header)
    if operation > 3 or count > MAX_SAMPLES or language_size > 32 or language_size + hints_size > MAX_METADATA:
        raise ValueError("Invalid local speech request limits")
    if (operation in (0, 2) and not count) or (operation == 1 and count):
        raise ValueError("Invalid local speech operation")
    language = exact(reader, language_size).decode("utf-8")
    if not re.fullmatch(r"[a-z-]*", language):
        raise ValueError("Invalid local speech language")
    hints = json.loads(exact(reader, hints_size).decode("utf-8")) if hints_size else []
    if not isinstance(hints, list) or any(not isinstance(value, str) for value in hints):
        raise ValueError("Dictionary hints must be a string array")
    samples = array.array("f")
    samples.frombytes(exact(reader, count * 4))
    if sys.byteorder != "little":
        samples.byteswap()
    if any(not math.isfinite(value) or abs(value) > 1 for value in samples):
        raise ValueError("Audio must contain normalized finite samples")
    return operation, samples, language, hints


def respond(writer, status, text):
    data = text.encode("utf-8")
    if len(data) > MAX_RESPONSE:
        status, data = 1, b"Local speech response exceeded its limit"
    writer.write(struct.pack("<2I", status, len(data)))
    writer.write(data)
    writer.flush()


def terminal(tokens, eos):
    ids = {eos} if isinstance(eos, int) else set(eos or [])
    if not len(tokens) or int(tokens[-1]) not in ids:
        raise ValueError("Local transcription was incomplete. Try a shorter recording.")


def structured_text(raw, partial=False):
    """Vibe outputs speaker-attributed JSON; never paste metadata or a broken JSON string."""
    raw = raw.strip()
    if raw.startswith("```"):
        raw = re.sub(r"^```(?:json)?\s*", "", raw)
        raw = re.sub(r"\s*```$", "", raw)
    if not raw:
        return ""
    if partial:
        # A content string can span multiple emitted chunks. Only complete JSON strings qualify.
        matches = re.finditer(r'"(?:Content|content|text)"\s*:\s*("(?:[^"\\]|\\.)*")', raw, re.S)
        return " ".join(json.loads(match[1]).strip() for match in matches).strip()
    decoder = json.JSONDecoder()
    items = []
    while raw:
        item, end = decoder.raw_decode(raw)
        items.extend(item if isinstance(item, list) else [item])
        raw = raw[end:].lstrip(" \t\r\n,")
    pieces = []
    for item in items:
        if not isinstance(item, dict):
            raise ValueError("Local speech model returned invalid transcription segments")
        content = next((item[key] for key in ("Content", "content", "text") if key in item), None)
        if not isinstance(content, str):
            raise ValueError("Local speech model returned invalid transcription content")
        pieces.append(content.strip())
    return " ".join(piece for piece in pieces if piece)


def streaming_text(raw, partial=False):
    """The streaming checkpoint emits line-anchored Speaker N: text, not JSON."""
    raw = raw.lstrip().replace("\r\n", "\n")
    if not raw:
        return ""
    header = re.compile(r"^[ \t]*Speaker[ \t]+[0-9]+[ \t]*:", re.M)
    prefix = re.compile(r"(?:S|Sp|Spe|Spea|Speak|Speake|Speaker)(?:[ \t]+[0-9]*[ \t]*)?")
    if not header.match(raw):
        if partial and prefix.fullmatch(raw.strip()):
            return ""
        raise ValueError("Local streaming model returned invalid speaker-attributed text")
    if "\n" in raw:
        before, _, tail = raw.rpartition("\n")
        if prefix.fullmatch(tail.strip()):
            if not partial:
                raise ValueError("Local streaming model returned an incomplete speaker label")
            raw = before  # A new speaker label can straddle emitted chunks; wait for its colon.
    return " ".join(piece.strip() for piece in header.split(raw) if piece.strip())


def append_text(text, next_text):
    """Discard an exact matching word boundary in Phi's overlapping audio chunks."""
    next_text = next_text.strip()
    if not text:
        return next_text
    before, after = text.split(), next_text.split()
    key = lambda word: re.sub(r"[^\w]", "", word).casefold()
    for size in range(min(16, len(before), len(after)), 0, -1):
        if [key(word) for word in before[-size:]] == [key(word) for word in after[:size]]:
            next_text = " ".join(after[size:])
            break
    return text + (" " + next_text if next_text else "")


def frame_config(path):
    config = json.loads((path / "preprocessor_config.json").read_text())
    rate = config["target_sample_rate"]
    ratio = config["speech_tok_compress_ratio"]
    chunk, delay = config["chunk_frames"], config["lookahead_frames"]
    if rate != 24_000 or not all(isinstance(value, int) for value in (ratio, chunk, delay)) or ratio <= 0 or chunk <= 0 or delay < 0:
        raise ValueError("Invalid VibeVoice streaming frame configuration")
    return chunk * ratio, delay * ratio


class Live:
    def __init__(self, engine):
        self.engine = engine
        self.chunk, self.lookahead = frame_config(engine.path)
        self.reset([])

    def reset(self, hints):
        self.samples = array.array("f")
        self.start = self.cursor = self.total = 0
        self.raw = ""
        self.finished = False
        with self.engine.torch.inference_mode():
            self.state = self.engine.model.init_streaming_state(self.engine.processor.tokenizer, context_info=", ".join(hints) or None)

    def append(self, samples, finish=False):
        if self.finished:
            raise ValueError("The local live session has already finished")
        self.total += len(samples)
        if self.total > MAX_SAMPLES:
            raise ValueError("Local live transcription is limited to 30 minutes")
        self.samples.extend(samples)
        total_out = (self.total * 3 + 1) // 2
        while self.cursor < total_out:
            end = self.cursor + self.chunk + self.lookahead
            if not finish and end * 2 > self.total * 3:
                break
            pcm = self.engine.resample(self.samples, self.cursor, min(end, total_out), self.start)
            if len(pcm) < self.chunk + self.lookahead:
                pcm = self.engine.np.pad(pcm, (0, self.chunk + self.lookahead - len(pcm)))
            with self.engine.torch.inference_mode():
                features = self.engine.model.encode_speech(self.engine.torch.from_numpy(pcm).to(self.engine.device).unsqueeze(0))
                text, self.state = self.engine.model.streaming_generate_step(features, self.state, self.engine.processor.tokenizer, max_new_tokens=512, temperature=0.0)
            if not self.state.pop("openglaido_complete", False):
                raise ValueError("Local live transcription was incomplete. Try a shorter recording.")
            self.raw += text
            if len(self.raw.encode("utf-8")) > MAX_RESPONSE:
                raise ValueError("Local live transcription exceeded its response limit")
            self.cursor += self.chunk
            discard = min(len(self.samples), max(0, self.cursor * 2 // 3 - self.start))
            del self.samples[:discard]
            self.start += discard
        if finish:
            self.finished = True
            self.state = None
            return streaming_text(self.raw)
        return streaming_text(self.raw, partial=True)


class Engine:
    def __init__(self, family, path):
        # Only bundled, pinned code and verified local model files are allowed. No downloads.
        os.environ.update(HF_HUB_OFFLINE="1", TRANSFORMERS_OFFLINE="1", HF_HUB_DISABLE_TELEMETRY="1", PYTORCH_ENABLE_MPS_FALLBACK="1")
        import numpy as np
        import torch
        self.np, self.torch = np, torch
        self.family, self.path = family, path
        self.device = "mps" if torch.backends.mps.is_available() else "cpu"
        if family == "phi4-multimodal":
            from transformers import GPT2TokenizerFast, GenerationConfig
            from phi4.configuration_phi4mm import Phi4MMConfig
            from phi4.modeling_phi4mm import Phi4MMForCausalLM
            from phi4.processing_phi4mm import Phi4MMProcessor, Phi4MMImageProcessor, Phi4MMAudioFeatureExtractor
            # Explicit bundled classes ignore checkpoint auto_map and never execute downloaded code.
            self.processor = Phi4MMProcessor(
                Phi4MMImageProcessor.from_pretrained(path, local_files_only=True),
                Phi4MMAudioFeatureExtractor.from_pretrained(path, local_files_only=True),
                GPT2TokenizerFast.from_pretrained(path, local_files_only=True),
            )
            config = Phi4MMConfig.from_pretrained(path, local_files_only=True)
            self.model = Phi4MMForCausalLM.from_pretrained(path, config=config, local_files_only=True, use_safetensors=True, torch_dtype=torch.float32, attn_implementation="eager").to(self.device).eval()
            self.generation = GenerationConfig.from_pretrained(path, local_files_only=True)
        else:
            from vibevoice.modular.modeling_vibevoice_asr import VibeVoiceASRForConditionalGeneration
            from vibevoice.processor.vibevoice_asr_processor import VibeVoiceASRProcessor
            self.processor = VibeVoiceASRProcessor.from_pretrained(str(path), language_model_pretrained_name=str(path), local_files_only=True)
            self.model = VibeVoiceASRForConditionalGeneration.from_pretrained(path, local_files_only=True, use_safetensors=True, torch_dtype=torch.float32, attn_implementation="sdpa").to(self.device).eval()
            if family == "vibevoice-asr-streaming" and self.processor.tokenizer.text_chunk_end_id is None:
                raise ValueError("The local streaming tokenizer is incomplete")
        self.live = Live(self) if family == "vibevoice-asr-streaming" else None

    def resample(self, samples, start=0, end=None, source_start=0):
        end = (len(samples) * 3 + 1) // 2 if end is None else end
        positions = self.np.arange(start, end, dtype=self.np.float64) * (2 / 3) - source_start
        return self.np.interp(positions, self.np.arange(len(samples)), self.np.asarray(samples, dtype=self.np.float32)).astype(self.np.float32)

    def batch(self, samples, hints):
        if self.live:
            self.live.reset(hints)
            return self.live.append(samples, finish=True)
        if self.family == "phi4-multimodal":
            result, offset = "", 0
            while offset < len(samples):
                end = min(len(samples), offset + RATE * 30)
                audio = self.np.asarray(samples[offset:end], dtype=self.np.float32)
                inputs = self.processor(text="<|user|><|audio_1|>Transcribe the audio clip into text.<|end|><|assistant|>", audios=[(audio, RATE)], return_tensors="pt").to(self.device)
                with self.torch.inference_mode():
                    output = self.model.generate(**inputs, generation_config=self.generation, max_new_tokens=2048, do_sample=False, num_beams=1, num_logits_to_keep=1)
                tokens = output[0, inputs["input_ids"].shape[1]:]
                terminal(tokens, self.generation.eos_token_id)
                result = append_text(result, self.processor.batch_decode(tokens.unsqueeze(0), skip_special_tokens=True, clean_up_tokenization_spaces=False)[0])
                if end == len(samples):
                    break
                offset = end - RATE
            return result
        audio = self.resample(samples)
        inputs = self.processor(audio=audio, sampling_rate=24_000, context_info=", ".join(hints) or None, return_tensors="pt", padding=True, add_generation_prompt=True)
        inputs = {key: value.to(self.device) if isinstance(value, self.torch.Tensor) else value for key, value in inputs.items()}
        budget = min(32768, max(512, math.ceil(len(samples) / RATE * 32) + 256))
        eos = self.processor.tokenizer.eos_token_id
        with self.torch.inference_mode():
            output = self.model.generate(**inputs, max_new_tokens=budget, do_sample=False, num_beams=1, pad_token_id=self.processor.pad_id, eos_token_id=eos)
        tokens = output[0, inputs["input_ids"].shape[1]:]
        terminal(tokens, eos)
        return structured_text(self.processor.decode(tokens, skip_special_tokens=True))

    def run(self, operation, samples, language, hints):
        if operation == 0:
            return self.batch(samples, hints)
        if self.live is None:
            raise ValueError("This local model does not support microphone streaming")
        if operation == 1:
            self.live.reset(hints)
            return ""
        return self.live.append(samples, finish=operation == 3)


def serve(engine, reader, writer):
    respond(writer, 0, "")
    while True:
        try:
            item = request(reader)
        except Exception as error:
            respond(writer, 1, str(error))
            return
        if item is None:
            return
        try:
            respond(writer, 0, engine.run(*item))
        except Exception as error:
            print(f"Microsoft speech inference failed: {error}", file=sys.stderr)
            respond(writer, 1, str(error))
            return


def self_test():
    for raw, expected in [('[{"Content":"Živjo"},{"Content":"svet"}]', "Živjo svet"), ('{"speaker":0,"content":"one"}{"speaker":1,"content":"two"}', "one two"), ('[]', "")]:
        assert structured_text(raw) == expected
    assert structured_text('{"content":"hello"},{"content":"wor', partial=True) == "hello"
    for raw in ['{"content":"unfinished', '{"speaker":0}', 'not a transcript']:
        try:
            structured_text(raw)
        except (ValueError, json.JSONDecodeError):
            pass
        else:
            raise AssertionError("Invalid structured transcription accepted")
    for raw, expected in [
        ("\n Speaker 0:And so, my fellow Americans.", "And so, my fellow Americans."),
        ("Speaker 0:First.\n Speaker 1:Second.", "First. Second."),
        ("Speaker 0:She said Speaker 1: hello.", "She said Speaker 1: hello."),
        ("Speaker 12:Živjo.\r\n\tSpeaker 3:こんにちは。", "Živjo. こんにちは。"),
        ("", ""),
        ("\n  ", ""),
    ]:
        assert streaming_text(raw) == expected
    for raw in ["S", "Spea", "Speaker", "Speaker ", "Speaker 12"]:
        assert streaming_text(raw, partial=True) == ""
    assert streaming_text("Speaker 0:Hello.\nSpeaker 1", partial=True) == "Hello."
    assert streaming_text("Speaker 0:Hello.\nSpeaker 1:World", partial=True) == "Hello. World"
    for raw in ["Unlabelled prose", "assistant\nSpeaker 0:Text", "Speaker 0", "Speaker 0:Hello.\nSpeaker 1", "Speaker 0:Hello.\nSpeaker", "Speaker 0:Hello.\nS"]:
        try:
            streaming_text(raw)
        except ValueError:
            pass
        else:
            raise AssertionError("Invalid streaming format accepted")
    assert append_text("café demain", "demain à Paris") == "café demain à Paris"
    for tokens, eos in [([1, 99], 99), ([99], [99, 100])]:
        terminal(tokens, eos)
    try:
        terminal([1, 2], [99])
    except ValueError:
        pass
    else:
        raise AssertionError("Incomplete generation accepted")
    hints = '["Tauri", "Živjo"]'.encode()
    encoded = struct.pack("<4I", 0, 2, 2, len(hints)) + b"sl" + hints + struct.pack("<2f", -1, 1)
    operation, samples, language, words = request(io.BytesIO(encoded))
    assert operation == 0 and samples.tolist() == [-1, 1] and language == "sl" and words == ["Tauri", "Živjo"]
    assert request(io.BytesIO()) is None
    for malformed in [encoded[:-1], struct.pack("<4I", 4, 0, 0, 0), struct.pack("<4I", 0, MAX_SAMPLES + 1, 0, 0), struct.pack("<4I", 0, 1, 0, 0) + struct.pack("<f", float("nan")), struct.pack("<4I", 1, 0, 0, 2) + b"{}"]:
        try:
            request(io.BytesIO(malformed))
        except ValueError:
            pass
        else:
            raise AssertionError("Invalid request accepted")
    class Echo:
        def run(self, *_): return "Živjo"
    output = io.BytesIO()
    serve(Echo(), io.BytesIO(encoded), output)
    data = output.getvalue()
    assert data[:8] == struct.pack("<2I", 0, 0)
    assert data[8:16] == struct.pack("<2I", 0, len("Živjo".encode()))
    assert data[16:].decode() == "Živjo"
    # Verify microphone framing/finalization without importing Torch or downloading a model.
    from contextlib import nullcontext
    from tempfile import TemporaryDirectory
    class Tensor(list):
        def to(self, _): return self
        def unsqueeze(self, _): return self
    class Torch:
        inference_mode = staticmethod(nullcontext)
        from_numpy = staticmethod(lambda values: Tensor(values))
    class Numpy:
        pad = staticmethod(lambda values, sizes: Tensor(values) + [0] * sizes[1])
    class Model:
        def __init__(self): self.calls, self.complete = [], True
        def init_streaming_state(self, *_args, **kwargs):
            self.calls.clear()
            return {"hints": kwargs.get("context_info")}
        def encode_speech(self, tensor):
            self.calls.append(len(tensor))
            return tensor
        def streaming_generate_step(self, _features, state, _tokenizer, **_kwargs):
            state["openglaido_complete"] = self.complete
            return f"\nSpeaker 0:{len(self.calls)}", state
    class Processor:
        tokenizer = object()
    class FakeEngine:
        torch, np, processor = Torch(), Numpy(), Processor()
        device = "cpu"
        def __init__(self, path): self.path, self.model = path, Model()
        def resample(self, _samples, start, end, _source_start): return Tensor([0] * (end - start))
    with TemporaryDirectory(prefix="openglaido-live-check-") as directory:
        path = Path(directory)
        (path / "preprocessor_config.json").write_text(json.dumps({"target_sample_rate": 24000, "speech_tok_compress_ratio": 3200, "chunk_frames": 1, "lookahead_frames": 1}))
        engine = FakeEngine(path)
        live = Live(engine)
        live.reset(["Tauri", "Živjo"])
        assert live.state["hints"] == "Tauri, Živjo"
        assert live.append(array.array("f", [0] * 4000)) == ""
        assert not engine.model.calls  # 6000 target-rate samples < 6400 chunk + lookahead.
        assert live.append(array.array("f", [0] * 1000)) == "1"
        assert engine.model.calls == [6400]
        assert live.append(array.array("f"), finish=True) == "1 2 3"
        assert engine.model.calls == [6400, 6400, 6400]
        assert live.state is None and live.finished
        try:
            live.append(array.array("f"), finish=True)
        except ValueError:
            pass
        else:
            raise AssertionError("The final chunk was committed twice")
        live.reset([])
        assert live.append(array.array("f"), finish=True) == ""
        assert not engine.model.calls
        live.reset([])
        engine.model.complete = False
        try:
            live.append(array.array("f", [0] * 5000))
        except ValueError:
            pass
        else:
            raise AssertionError("An incomplete live chunk was accepted")
    print("Microsoft helper protocol self-tests passed")


def main():
    if sys.argv[1:] == ["--self-test"]:
        self_test()
        return
    sys.path.insert(0, str(Path(__file__).resolve().parent / "vendor"))
    if sys.argv[1:] == ["--check-runtime"]:
        os.environ.update(HF_HUB_OFFLINE="1", TRANSFORMERS_OFFLINE="1", HF_HUB_DISABLE_TELEMETRY="1", PYTORCH_ENABLE_MPS_FALLBACK="1")
        import torch
        import transformers
        import peft
        import diffusers
        from vibevoice.modular.modeling_vibevoice_asr import VibeVoiceASRForConditionalGeneration
        from vibevoice.processor.vibevoice_asr_processor import VibeVoiceASRProcessor
        from phi4.configuration_phi4mm import Phi4MMConfig
        from phi4.modeling_phi4mm import Phi4MMForCausalLM
        from phi4.processing_phi4mm import Phi4MMProcessor, Phi4MMImageProcessor, Phi4MMAudioFeatureExtractor
        assert all(value for value in (VibeVoiceASRForConditionalGeneration, VibeVoiceASRProcessor, Phi4MMConfig, Phi4MMForCausalLM, Phi4MMProcessor, Phi4MMImageProcessor, Phi4MMAudioFeatureExtractor))
        # Exercise the actual processor and tensor batching without model weights or GPU inference.
        import numpy as np
        class PromptTokenizer:
            speech_start_id, speech_end_id, speech_pad_id, pad_id = 10, 11, 12, 0
            def convert_ids_to_tokens(self, value): return {10: "<sp_start>", 11: "<sp_end>", 12: "<sp_pad>"}[value]
            def apply_chat_template(self, messages, tokenize=False):
                return [10] + [12] * messages[0]["content"].count("<sp_pad>") + [11] if tokenize else "system"
            def encode(self, text, **_kwargs): return [20, 21, 22] if text == "<|im_start|>assistant\n" else [30, 31]
        processor = VibeVoiceASRProcessor(tokenizer=PromptTokenizer(), normalize_audio=False)
        audio = np.zeros(3200, dtype=np.float32)
        plain = processor._process_single_audio(audio, add_generation_prompt=False)
        prompt = processor._process_single_audio(audio, add_generation_prompt=True)
        assert prompt["input_ids"] == plain["input_ids"] + [20, 21, 22]
        assert prompt["acoustic_input_mask"] == plain["acoustic_input_mask"] + [0, 0, 0]
        batch = processor(audio=[audio, np.zeros(6400, dtype=np.float32)], return_tensors="pt", add_generation_prompt=True)
        assert batch["input_ids"][:, -3:].tolist() == [[20, 21, 22], [20, 21, 22]]
        assert batch["acoustic_input_mask"][:, -3:].tolist() == [[False, False, False], [False, False, False]]
        assert batch["attention_mask"][0].sum().item() == len(prompt["input_ids"])
        assert batch["attention_mask"][:, -3:].tolist() == [[1, 1, 1], [1, 1, 1]]
        print(json.dumps({"torch": torch.__version__, "transformers": transformers.__version__, "peft": peft.__version__, "diffusers": diffusers.__version__, "device": "mps" if torch.backends.mps.is_available() else "cpu"}))
        return
    if len(sys.argv) != 3 or sys.argv[1] not in FAMILIES or not Path(sys.argv[2]).is_dir():
        raise SystemExit("Usage: openglaido-microsoft FAMILY MODEL_DIR")
    # Redirect file-descriptor stdout too: native libraries sometimes print outside Python's logger.
    writer = os.fdopen(os.dup(sys.stdout.fileno()), "wb", buffering=0)
    os.dup2(sys.stderr.fileno(), sys.stdout.fileno())
    try:
        engine = Engine(sys.argv[1], Path(sys.argv[2]).resolve())
    except Exception as error:
        respond(writer, 1, str(error))
        return
    serve(engine, sys.stdin.buffer, writer)


if __name__ == "__main__":
    # PyInstaller's hook dispatches spawned workers/resource trackers before our app CLI.
    import multiprocessing
    multiprocessing.freeze_support()
    main()
