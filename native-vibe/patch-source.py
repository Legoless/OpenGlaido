#!/usr/bin/env python3
"""Apply the smallest I/O and completeness changes to the pinned Microsoft server."""
import pathlib
import sys

source = pathlib.Path(sys.argv[1]).read_text()

def replace(old, new):
    global source
    if source.count(old) != 1:
        raise SystemExit(f"Pinned VibeVoice source no longer matches patch: {old[:80]}")
    source = source.replace(old, new)

replace('#include "vae.h"', '#include "protocol.h"\n#include "vae.h"')
replace('#include "../utils/audio_io.h"', '#include "audio_io.h"')
replace('#include "../utils/prompt_builder.h"', '#include "prompt_builder.h"')
replace('int main(int argc, char ** argv) {\n    server_params params;', '''int main(int argc, char ** argv) {
    if (argc == 2 && std::string(argv[1]) == "--self-test") return vibe_self_test();
    server_params params;''')
replace('fprintf(stdout, "[ERROR] Failed to load audio: %s\\n---END---\\n", audio_path.c_str());',
        'vibe_reply(1, "Failed to load recorded audio");')
for error in ['VAE acoustic encoding failed', 'VAE semantic encoding failed', 'Failed to build prompt', 'Segmented prefill failed']:
    replace(f'fprintf(stdout, "[ERROR] {error}\\n---END---\\n");', f'vibe_reply(1, "{error}");')
replace('int n_prompt_tokens = (int)prompt.tokens.size();', '''int n_prompt_tokens = (int)prompt.tokens.size();
    if (n_prompt_tokens >= params.n_ctx - 1024) {
        vibe_reply(1, "Recording exceeds the local model context. Use a shorter recording.");
        return -1;
    }''')
replace('llama_sampler_free(smpl);\n\n    // Non-streaming mode: output all at once', '''llama_sampler_free(smpl);

    if (!vibe_complete(new_token, EOG_IM_END, EOG_ENDOFTEXT)) {
        vibe_reply(1, "The local model could not finish the entire transcript. Use a shorter recording.");
        return -1;
    }

    // Non-streaming mode: output all at once''')
replace('fprintf(stdout, "%s", output_text.c_str());', 'vibe_reply(0, output_text);')
replace('fprintf(stdout, "\\n---END---\\n");', '// The framed response above already contains the complete result.')
replace('fprintf(stdout, "---READY---\\n");', 'vibe_reply(0, "");')
# Two identical acknowledgments deliberately share the same framing contract.
assert source.count('fprintf(stdout, "---ACK---\\n");') == 2
source = source.replace('fprintf(stdout, "---ACK---\\n");', 'vibe_reply(0, "");')
# The integration requests complete text; never expose a token stream on the framed pipe.
replace('if (!parse_args(argc, argv, params)) {\n        return 1;\n    }', '''if (!parse_args(argc, argv, params)) {
        return 1;
    }
    params.token_stream = false;''')
pathlib.Path(sys.argv[2]).write_text(source)
