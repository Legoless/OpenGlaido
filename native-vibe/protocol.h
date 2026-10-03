#pragma once
#include <cstdlib>
#include <cstdio>
#include <cstdint>
#include <cstring>
#include <string>

constexpr uint32_t vibe_max_reply = 4 * 1024 * 1024;

// Responses are framed rather than split on tokens/newlines: transcript text cannot impersonate
// the upstream server's control markers, and UTF-8 remains intact across token boundaries.
static void vibe_reply(uint32_t status, const std::string &text) {
    if (status > 1 || text.size() > vibe_max_reply) {
        status = 1;
        return vibe_reply(status, "The transcription response is too large");
    }
    const uint32_t fields[] = {status, static_cast<uint32_t>(text.size())};
    unsigned char header[8];
    for (size_t field = 0; field < 2; ++field) {
        for (size_t byte = 0; byte < 4; ++byte) header[field * 4 + byte] = static_cast<unsigned char>(fields[field] >> (8 * byte));
    }
    if (fwrite(header, 1, sizeof(header), stdout) != sizeof(header) ||
        fwrite(text.data(), 1, text.size(), stdout) != text.size() || fflush(stdout) != 0) std::exit(1);
}

static bool vibe_complete(int token, int end, int eos) { return token == end || token == eos; }

static void vibe_require(bool condition) { if (!condition) { fprintf(stderr, "VibeVoice self-test failed\n"); std::exit(2); } }

static int vibe_self_test() {
    vibe_require(vibe_complete(151645, 151645, 151643));
    vibe_require(vibe_complete(151643, 151645, 151643));
    vibe_require(!vibe_complete(42, 151645, 151643));
    // Completion must not accept an exhausted output budget as a successful partial transcript.
    const uint32_t length = 0x01020304;
    unsigned char little[] = {static_cast<unsigned char>(length), static_cast<unsigned char>(length >> 8),
        static_cast<unsigned char>(length >> 16), static_cast<unsigned char>(length >> 24)};
    vibe_require(little[0] == 4 && little[1] == 3 && little[2] == 2 && little[3] == 1);
    fprintf(stderr, "OpenGlaido VibeVoice protocol checks passed\n");
    return 0;
}
