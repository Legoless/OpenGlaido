#include <transcribe.h>

#include <algorithm>
#include <cctype>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <memory>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
constexpr uint32_t sample_rate = 16000;
constexpr uint32_t max_samples = sample_rate * 30 * 60;
constexpr size_t max_response = 4 * 1024 * 1024;
constexpr size_t overlap_samples = sample_rate * 3 / 4;

void require(bool condition, const char *message) {
    if (!condition) throw std::runtime_error(message);
}

uint32_t read_u32(std::istream &input) {
    unsigned char bytes[4];
    require(bool(input.read(reinterpret_cast<char *>(bytes), 4)), "Incomplete request header");
    return uint32_t(bytes[0]) | (uint32_t(bytes[1]) << 8) | (uint32_t(bytes[2]) << 16) | (uint32_t(bytes[3]) << 24);
}

void write_u32(std::ostream &output, uint32_t value) {
    const char bytes[] = {char(value), char(value >> 8), char(value >> 16), char(value >> 24)};
    output.write(bytes, sizeof(bytes));
}

void respond(std::ostream &output, uint32_t status, const std::string &text) {
    require(text.size() <= max_response, "Transcription response is too large");
    write_u32(output, status);
    write_u32(output, static_cast<uint32_t>(text.size()));
    output.write(text.data(), static_cast<std::streamsize>(text.size()));
    output.flush();
    require(bool(output), "Could not write transcription response");
}

struct Request {
    std::string language;
    std::vector<float> pcm;
};

Request read_request(std::istream &input) {
    const auto count = read_u32(input);
    const auto language_size = read_u32(input);
    require(count > 0 && count <= max_samples, "Recording must be between one sample and 30 minutes");
    require(language_size <= 32, "Invalid language length");
    Request request;
    request.language.resize(language_size);
    require(bool(input.read(request.language.data(), language_size)), "Incomplete language");
    require(std::all_of(request.language.begin(), request.language.end(), [](unsigned char ch) {
        return (ch >= 'a' && ch <= 'z') || ch == '-';
    }), "Language must be a lowercase language code");
    // macOS arm64 and x86_64 both use IEEE-754 little-endian float32.
    static_assert(sizeof(float) == 4);
    request.pcm.resize(count);
    require(bool(input.read(reinterpret_cast<char *>(request.pcm.data()), count * sizeof(float))), "Incomplete audio");
    require(std::all_of(request.pcm.begin(), request.pcm.end(), [](float value) {
        return std::isfinite(value) && std::abs(value) <= 1.0f;
    }), "Audio must contain finite normalized float32 samples");
    return request;
}

struct Word { size_t start, end; std::string key; };

std::vector<Word> words(const std::string &text) {
    std::vector<Word> result;
    for (size_t index = 0; index < text.size();) {
        while (index < text.size() && std::isspace(static_cast<unsigned char>(text[index]))) ++index;
        Word word{index, index, {}};
        while (index < text.size() && !std::isspace(static_cast<unsigned char>(text[index]))) {
            const auto ch = static_cast<unsigned char>(text[index++]);
            if (ch >= 128 || std::isalnum(ch)) word.key += ch < 128 ? char(std::tolower(ch)) : char(ch);
        }
        word.end = index;
        if (!word.key.empty()) result.push_back(std::move(word));
    }
    return result;
}

std::string trim(std::string text) {
    const auto first = text.find_first_not_of(" \t\n\r");
    if (first == std::string::npos) return {};
    return text.substr(first, text.find_last_not_of(" \t\n\r") - first + 1);
}

bool unspaced_script(const std::string &text, bool last) {
    size_t offset = last ? text.size() - 1 : 0;
    while (offset > 0 && (static_cast<unsigned char>(text[offset]) & 0xc0) == 0x80) --offset;
    const auto first = static_cast<unsigned char>(text[offset]);
    const size_t width = first < 0x80 ? 1 : first < 0xe0 ? 2 : first < 0xf0 ? 3 : 4;
    if (offset + width > text.size()) return false;
    uint32_t codepoint = first & (0x7f >> width);
    for (size_t index = 1; index < width; ++index) {
        const auto continuation = static_cast<unsigned char>(text[offset + index]);
        if ((continuation & 0xc0) != 0x80) return false;
        codepoint = (codepoint << 6) | (continuation & 0x3f);
    }
    return (codepoint >= 0x3000 && codepoint <= 0x30ff) || // CJK punctuation, kana
           (codepoint >= 0x31f0 && codepoint <= 0x31ff) ||
           (codepoint >= 0x3400 && codepoint <= 0x9fff) || // Han
           (codepoint >= 0xf900 && codepoint <= 0xfaff) ||
           (codepoint >= 0x20000 && codepoint <= 0x323af) ||
           (codepoint >= 0x0e00 && codepoint <= 0x0eff) || // Thai, Lao
           (codepoint >= 0x1000 && codepoint <= 0x109f) || // Myanmar
           (codepoint >= 0x1780 && codepoint <= 0x17ff);   // Khmer
}

void append_transcript(std::string &text, const std::string &next_text) {
    const auto next = trim(next_text);
    if (next.empty()) return;
    if (text.empty()) { text = next; return; }
    const auto previous_words = words(text);
    const auto next_words = words(next);
    size_t start = 0;
    // The audio overlaps by 750 ms; only discard an exact matching boundary.
    for (size_t count = std::min({size_t(16), previous_words.size(), next_words.size()}); count > 0; --count) {
        bool matches = true;
        for (size_t index = 0; index < count; ++index) {
            if (previous_words[previous_words.size() - count + index].key != next_words[index].key) matches = false;
        }
        if (matches) { start = next_words[count - 1].end; break; }
    }
    if (start == 0 && next_words.size() == 1 && previous_words.size() == 1 &&
        unspaced_script(text, true) && unspaced_script(next, false)) {
        // Scripts without spaces: match complete UTF-8 suffix/prefix bytes.
        for (size_t count = std::min({size_t(96), text.size(), next.size()}); count >= 6; --count) {
            if ((static_cast<unsigned char>(next[count]) & 0xc0) != 0x80 &&
                text.compare(text.size() - count, count, next, 0, count) == 0) {
                start = count;
                break;
            }
        }
    }
    const auto remainder = trim(next.substr(start));
    if (!remainder.empty()) {
        if (!unspaced_script(text, true) || !unspaced_script(remainder, false)) text += ' ';
        text += remainder;
    }
    require(text.size() <= max_response, "Transcription response is too large");
}

size_t chunk_end(const std::vector<float> &pcm, size_t offset, size_t limit) {
    const size_t end = std::min(pcm.size(), offset + limit);
    if (end == pcm.size() || limit < sample_rate * 2) return end;
    const size_t window = sample_rate / 20;
    const size_t search_start = end - std::min(size_t(sample_rate * 5), limit / 4);
    size_t boundary = end;
    double lowest = INFINITY;
    for (size_t start = search_start; start + window <= end; start += window) {
        double energy = 0;
        for (size_t index = start; index < start + window; ++index) energy += double(pcm[index]) * pcm[index];
        if (energy <= lowest) { lowest = energy; boundary = start + window / 2; }
    }
    return boundary;
}

void self_test() {
    std::ostringstream frame;
    write_u32(frame, 2); write_u32(frame, 2); frame << "en";
    const float samples[] = {0.25f, -0.5f};
    frame.write(reinterpret_cast<const char *>(samples), sizeof(samples));
    std::istringstream input(frame.str());
    const auto request = read_request(input);
    require(request.language == "en" && request.pcm == std::vector<float>({0.25f, -0.5f}), "PCM framing test failed");
    for (const auto &invalid : {std::string("x"), std::string(8, '\0'), frame.str().substr(0, 12)}) {
        bool rejected = false;
        try { std::istringstream malformed(invalid); read_request(malformed); }
        catch (const std::runtime_error &) { rejected = true; }
        require(rejected, "Invalid request was accepted");
    }
    auto nonfinite = frame.str();
    const uint32_t nan = 0x7fc00000;
    std::memcpy(nonfinite.data() + 10, &nan, sizeof(nan));
    bool rejected = false;
    try { std::istringstream malformed(nonfinite); read_request(malformed); }
    catch (const std::runtime_error &) { rejected = true; }
    require(rejected, "NaN audio was accepted");
    auto invalid_language = frame.str();
    invalid_language[8] = '\n';
    rejected = false;
    try { std::istringstream malformed(invalid_language); read_request(malformed); }
    catch (const std::runtime_error &) { rejected = true; }
    require(rejected, "Invalid language was accepted");
    std::ostringstream oversized;
    write_u32(oversized, max_samples + 1); write_u32(oversized, 0);
    rejected = false;
    try { std::istringstream malformed(oversized.str()); read_request(malformed); }
    catch (const std::runtime_error &) { rejected = true; }
    require(rejected, "Oversized recording was accepted");
    std::string text = "This is a test.";
    append_transcript(text, "A test, followed by more speech.");
    require(text == "This is a test. followed by more speech.", "Overlap test failed");
    append_transcript(text, "Completely different.");
    require(text == "This is a test. followed by more speech. Completely different.", "Append test failed");
    std::string chinese = "我们今天学习中文";
    append_transcript(chinese, "学习中文然后继续");
    require(chinese == "我们今天学习中文然后继续", "UTF-8 overlap test failed");
    for (const auto &parts : {std::vector<std::string>{"café", "été prochain"},
                              std::vector<std::string>{"καλημέρα", "κόσμε"},
                              std::vector<std::string>{"доброе", "утро"},
                              std::vector<std::string>{"안녕하세요", "여러분"},
                              std::vector<std::string>{"bibliothèque", "thèque"}}) {
        auto joined = parts[0];
        append_transcript(joined, parts[1]);
        require(joined == parts[0] + " " + parts[1], "Unicode word separator test failed");
    }
    std::vector<float> long_pcm(sample_rate * 65, 0.2f);
    std::fill(long_pcm.begin() + sample_rate * 28, long_pcm.begin() + sample_rate * 28 + sample_rate / 20, 0.0f);
    const auto boundary = chunk_end(long_pcm, 0, sample_rate * 30);
    require(boundary == sample_rate * 28 + sample_rate / 40, "Quiet boundary test failed");
    size_t offset = 0;
    int chunks = 0;
    while (offset < long_pcm.size()) {
        const auto end = chunk_end(long_pcm, offset, sample_rate * 30);
        require(end > offset && end - offset <= sample_rate * 30, "Chunk progression failed");
        if (end == long_pcm.size()) break;
        offset = end - overlap_samples;
        require(++chunks < 10, "Chunk loop did not finish");
    }
    std::ostringstream response;
    respond(response, 0, "Hello");
    std::istringstream decoded(response.str());
    require(read_u32(decoded) == 0 && read_u32(decoded) == 5, "Response framing test failed");
}

int serve(const char *path) {
    transcribe_model *raw_model = nullptr;
    auto status = transcribe_model_load_file(path, nullptr, &raw_model);
    require(status == TRANSCRIBE_OK, transcribe_status_string(status));
    const std::unique_ptr<transcribe_model, decltype(&transcribe_model_free)> model(raw_model, transcribe_model_free);
    transcribe_session *raw_session = nullptr;
    status = transcribe_session_init(model.get(), nullptr, &raw_session);
    require(status == TRANSCRIBE_OK, transcribe_status_string(status));
    const std::unique_ptr<transcribe_session, decltype(&transcribe_session_free)> session(raw_session, transcribe_session_free);
    transcribe_capabilities capabilities;
    transcribe_capabilities_init(&capabilities);
    status = transcribe_model_get_capabilities(model.get(), &capabilities);
    require(status == TRANSCRIBE_OK, transcribe_status_string(status));
    const bool explicit_language = std::strcmp(transcribe_model_arch_string(model.get()), "cohere") == 0;
    size_t limit = sample_rate * 30;
    if (capabilities.max_audio_ms > 0) {
        limit = std::min(limit, static_cast<size_t>(capabilities.max_audio_ms) * sample_rate * 9 / 10000);
    }
    require(limit > overlap_samples * 2, "Model audio window is too small");
    respond(std::cout, 0, {});
    while (std::cin.peek() != std::char_traits<char>::eof()) {
        const auto request = read_request(std::cin);
        if (explicit_language && request.language.empty()) {
            respond(std::cout, 1, "Cohere Transcribe requires a transcription language");
            continue;
        }
        transcribe_run_params params;
        transcribe_run_params_init(&params);
        params.language = request.language.empty() ? nullptr : request.language.c_str();
        params.timestamps = TRANSCRIBE_TIMESTAMPS_NONE;
        std::string transcript;
        for (size_t offset = 0; offset < request.pcm.size();) {
            const auto end = chunk_end(request.pcm, offset, limit);
            status = transcribe_run(session.get(), request.pcm.data() + offset, static_cast<int>(end - offset), &params);
            if (status != TRANSCRIBE_OK) break;
            append_transcript(transcript, transcribe_full_text(session.get()));
            if (end == request.pcm.size()) break;
            offset = end - overlap_samples;
        }
        if (status == TRANSCRIBE_OK) respond(std::cout, 0, transcript);
        else respond(std::cout, 1, transcribe_status_string(status));
    }
    return 0;
}
} // namespace

int main(int argc, char **argv) {
    try {
        if (argc == 2 && std::strcmp(argv[1], "--self-test") == 0) { self_test(); return 0; }
        require(argc == 2, "Usage: openglaido-stt <model.gguf>");
        if (__builtin_available(macOS 12.0, *)) return serve(argv[1]);
        throw std::runtime_error("These local speech models require macOS 12 or newer");
    } catch (const std::exception &error) {
        try { respond(std::cout, 1, error.what()); } catch (...) {}
        return 1;
    } catch (...) {
        try { respond(std::cout, 1, "Native transcription failed"); } catch (...) {}
        return 1;
    }
}
