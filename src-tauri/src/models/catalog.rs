//! Downloadable local models. Sizes and SHA-256 come from the Hugging Face API (verified 2026-09-30; native STT additions 2026-10-02); URLs pin
//! the commit they were taken from, so an upstream re-upload can't break downloads.

use super::CatalogModel;

pub const CATALOG: &[CatalogModel] = &[
    CatalogModel {
        id: "whisper-tiny",
        kind: "stt",
        backend: "whisper",
        name: "Tiny",
        notes: "Fastest, lowest accuracy. Good for quick tests.",
        file: "ggml-tiny.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-tiny.bin",
        size_bytes: 77691713,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        english_only: false,
        speed: 5,
        accuracy: 1,
        recommended: false,
        license: "MIT",
        template: "",
    },
    CatalogModel {
        id: "whisper-tiny-en",
        kind: "stt",
        backend: "whisper",
        name: "Tiny (English)",
        notes: "Fastest, English only.",
        file: "ggml-tiny.en.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-tiny.en.bin",
        size_bytes: 77704715,
        sha256: "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f",
        english_only: true,
        speed: 5,
        accuracy: 1,
        recommended: false,
        license: "MIT",
        template: "",
    },
    CatalogModel {
        id: "whisper-base",
        kind: "stt",
        backend: "whisper",
        name: "Base",
        notes: "Very fast, basic accuracy.",
        file: "ggml-base.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.bin",
        size_bytes: 147951465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        english_only: false,
        speed: 4,
        accuracy: 2,
        recommended: false,
        license: "MIT",
        template: "",
    },
    CatalogModel {
        id: "whisper-base-en",
        kind: "stt",
        backend: "whisper",
        name: "Base (English)",
        notes: "Very fast, English only.",
        file: "ggml-base.en.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.en.bin",
        size_bytes: 147964211,
        sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
        english_only: true,
        speed: 4,
        accuracy: 2,
        recommended: false,
        license: "MIT",
        template: "",
    },
    CatalogModel {
        id: "whisper-small",
        kind: "stt",
        backend: "whisper",
        name: "Small",
        notes: "Fast with decent multilingual accuracy.",
        file: "ggml-small.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-small.bin",
        size_bytes: 487601967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        english_only: false,
        speed: 3,
        accuracy: 3,
        recommended: false,
        license: "MIT",
        template: "",
    },
    CatalogModel {
        id: "whisper-small-en",
        kind: "stt",
        backend: "whisper",
        name: "Small (English)",
        notes: "Fast, good English accuracy.",
        file: "ggml-small.en.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-small.en.bin",
        size_bytes: 487614201,
        sha256: "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
        english_only: true,
        speed: 3,
        accuracy: 3,
        recommended: false,
        license: "MIT",
        template: "",
    },
    CatalogModel {
        id: "whisper-large-v3-turbo-q5",
        kind: "stt",
        backend: "whisper",
        name: "Large v3 Turbo (quantized)",
        notes: "Best balance: near-large accuracy at a fraction of the size.",
        file: "ggml-large-v3-turbo-q5_0.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo-q5_0.bin",
        size_bytes: 574041195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        english_only: false,
        speed: 3,
        accuracy: 4,
        recommended: true,
        license: "MIT",
        template: "",
    },
    CatalogModel {
        id: "whisper-large-v3-turbo",
        kind: "stt",
        backend: "whisper",
        name: "Large v3 Turbo",
        notes: "Full-precision Turbo, slightly better than quantized.",
        file: "ggml-large-v3-turbo.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo.bin",
        size_bytes: 1624555275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        english_only: false,
        speed: 3,
        accuracy: 4,
        recommended: false,
        license: "MIT",
        template: "",
    },
    CatalogModel {
        id: "whisper-large-v3-q5",
        kind: "stt",
        backend: "whisper",
        name: "Large v3 (quantized)",
        notes: "Top accuracy, smaller download, slower.",
        file: "ggml-large-v3-q5_0.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-q5_0.bin",
        size_bytes: 1081140203,
        sha256: "d75795ecff3f83b5faa89d1900604ad8c780abd5739fae406de19f23ecd98ad1",
        english_only: false,
        speed: 2,
        accuracy: 5,
        recommended: false,
        license: "MIT",
        template: "",
    },
    CatalogModel {
        id: "whisper-large-v3",
        kind: "stt",
        backend: "whisper",
        name: "Large v3",
        notes: "Maximum accuracy, largest and slowest.",
        file: "ggml-large-v3.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3.bin",
        size_bytes: 3095033483,
        sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
        english_only: false,
        speed: 1,
        accuracy: 5,
        recommended: false,
        license: "MIT",
        template: "",
    },
    CatalogModel {
        id: "parakeet-ultra-q8",
        kind: "stt",
        backend: "native",
        name: "Parakeet Ultra 0.6B (Q8)",
        notes: "25 European languages, including Slovenian; detects the spoken language.",
        file: "parakeet-ultra-0.6b-Q8_0.gguf",
        url: "https://huggingface.co/Nairod785/parakeet-ultra-gguf/resolve/b03613ba54a195238f0e915359f5a5c78269ddc6/parakeet-ultra-0.6b-Q8_0.gguf",
        size_bytes: 739508704,
        sha256: "283562ac9b513f39244fe23c6632738c167d32731a5f4693319a10ca498550a8",
        english_only: false,
        speed: 0,
        accuracy: 0,
        recommended: false,
        license: "CC-BY-4.0",
        template: "",
    },
    CatalogModel {
        id: "ark-asr-3b-q8",
        kind: "stt",
        backend: "native",
        name: "ARK-ASR 3B (Q8)",
        notes: "Multilingual transcription with a larger download; detects the spoken language.",
        file: "ark-asr-3b-Q8_0.gguf",
        url: "https://huggingface.co/harshav/ARK-ASR-3B-GGUF/resolve/14239df600b76b1fd697423eb927f9f4622fe739/ark-asr-3b-Q8_0.gguf",
        size_bytes: 4289201792,
        sha256: "b9ab32cfe7982eed5a596a601059e112d4aa5476e851852f944062a10fb25ee8",
        english_only: false,
        speed: 0,
        accuracy: 0,
        recommended: false,
        license: "Apache-2.0",
        template: "",
    },
    CatalogModel {
        id: "qwen3-asr-1.7b-q8",
        kind: "stt",
        backend: "native",
        name: "Qwen3-ASR 1.7B (Q8)",
        notes: "30 languages with automatic detection.",
        file: "Qwen3-ASR-1.7B-Q8_0.gguf",
        url: "https://huggingface.co/handy-computer/Qwen3-ASR-1.7B-gguf/resolve/3555bd238a8572bbace3ebf60d23b036dc0a5dbe/Qwen3-ASR-1.7B-Q8_0.gguf",
        size_bytes: 2185030624,
        sha256: "9a0d81792dfea2d5f278b8a63deb3ea6e02139ce42c2301f32ea19c4f77526b7",
        english_only: false,
        speed: 0,
        accuracy: 0,
        recommended: false,
        license: "Apache-2.0",
        template: "",
    },
    CatalogModel {
        id: "cohere-transcribe-2b-q8",
        kind: "stt",
        backend: "native",
        name: "Cohere Transcribe 2B (Q8)",
        notes: "14 languages. Choose the spoken language; automatic detection is unavailable.",
        file: "cohere-transcribe-03-2026-Q8_0.gguf",
        url: "https://huggingface.co/handy-computer/cohere-transcribe-03-2026-gguf/resolve/ab667acedcb5d8d56837ef3ed3568c3ad50dde47/cohere-transcribe-03-2026-Q8_0.gguf",
        size_bytes: 2410655232,
        sha256: "931916663432fd895423a4291a8400221802b288967ca2d435fc5e3141c9e71e",
        english_only: false,
        speed: 0,
        accuracy: 0,
        recommended: false,
        license: "Apache-2.0",
        template: "",
    },
    CatalogModel {
        id: "voxtral-realtime-4b-q8",
        kind: "stt",
        backend: "native",
        name: "Voxtral Realtime 4B (Q8)",
        notes: "13 languages; transcribes after recording in OpenGlaido.",
        file: "Voxtral-Mini-4B-Realtime-2602-Q8_0.gguf",
        url: "https://huggingface.co/handy-computer/Voxtral-Mini-4B-Realtime-2602-gguf/resolve/65d1c9408859a0ca0f1c11b025ee951486af21d6/Voxtral-Mini-4B-Realtime-2602-Q8_0.gguf",
        size_bytes: 4731791648,
        sha256: "6fb22249463c1e7a600d920ad732e2dd0a231510e9db59fb8061d3d1d48d0c67",
        english_only: false,
        speed: 0,
        accuracy: 0,
        recommended: false,
        license: "Apache-2.0",
        template: "",
    },
    CatalogModel {
        id: "gemma-4-e2b-it-q4km",
        kind: "llm",
        backend: "llama",
        name: "Gemma 4 E2B Instruct",
        notes: "Most reliable cleanup: keeps your language and never answers what you dictate.",
        file: "gemma-4-E2B-it-Q4_K_M.gguf",
        url: "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/0314792d7f1f7e229411f620751375812bb9faf2/gemma-4-E2B-it-Q4_K_M.gguf",
        size_bytes: 3106738272,
        sha256: "740185b21d22ceb83a11c3aa62ad5842ef32c70f6096d756bbee85a1e4ec34b8",
        english_only: false,
        speed: 3,
        accuracy: 5,
        recommended: true,
        license: "Apache-2.0",
        template: "gemma4",
    },
    CatalogModel {
        id: "qwen3-4b-instruct-2507-q4km",
        kind: "llm",
        backend: "llama",
        name: "Qwen3 4B Instruct 2507",
        notes: "Strong punctuation and formatting; may turn some languages into English.",
        file: "Qwen_Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Qwen_Qwen3-4B-Instruct-2507-GGUF/resolve/ae44f08e1392f39c0e474af10c3ff8355c8b6688/Qwen_Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        size_bytes: 2497280736,
        sha256: "2fde00ce69dd4899c70d020845e2638353015bba0fdf161b3eb965f2bca4464e",
        english_only: false,
        speed: 3,
        accuracy: 4,
        recommended: false,
        license: "Apache-2.0",
        template: "chatml",
    },
    CatalogModel {
        id: "ministral-3-3b-instruct-2512-q4km",
        kind: "llm",
        backend: "llama",
        name: "Ministral 3 3B Instruct",
        notes: "Good with European languages; rewrites more freely.",
        file: "Ministral-3-3B-Instruct-2512-Q4_K_M.gguf",
        url: "https://huggingface.co/mistralai/Ministral-3-3B-Instruct-2512-GGUF/resolve/eb599d408350ea2bb60452cb86be7c7b2fc28227/Ministral-3-3B-Instruct-2512-Q4_K_M.gguf",
        size_bytes: 2147023008,
        sha256: "9ed150d4367e68df0ac8e1540f6ddc65b42d0ee26378329d1ecbca60f93fc5f8",
        english_only: false,
        speed: 3,
        accuracy: 3,
        recommended: false,
        license: "Apache-2.0",
        template: "mistral-v7-tekken",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn downloadable_models_have_unique_files_and_immutable_verified_sources() {
        let mut ids = HashSet::new();
        let mut files = HashSet::new();
        for model in CATALOG {
            assert!(ids.insert(model.id), "duplicate model id: {}", model.id);
            assert!(
                files.insert(model.file),
                "shared download file: {}",
                model.file
            );
            assert!(
                !model.file.contains(['/', '\\']),
                "unsafe filename: {}",
                model.file
            );
            assert!(model.size_bytes > 0);
            assert_eq!(model.sha256.len(), 64, "{}", model.id);
            assert!(model.sha256.bytes().all(|c| c.is_ascii_hexdigit()));
            let path = model
                .url
                .strip_prefix("https://huggingface.co/")
                .expect("HTTPS model source");
            let (repo, revision_and_file) = path.split_once("/resolve/").expect("pinned model URL");
            assert_eq!(repo.split('/').count(), 2);
            let (revision, file) = revision_and_file
                .split_once('/')
                .expect("revision and filename");
            assert_eq!(revision.len(), 40, "unpinned model: {}", model.id);
            assert!(revision.bytes().all(|c| c.is_ascii_hexdigit()));
            assert_eq!(file, model.file);
            assert!(matches!(
                (model.kind, model.backend),
                ("stt", "whisper" | "native") | ("llm", "llama")
            ));
        }
    }

    #[test]
    fn native_stt_catalog_contains_each_requested_family_without_unmeasured_ratings() {
        let native: Vec<_> = CATALOG.iter().filter(|m| m.backend == "native").collect();
        assert_eq!(
            native.iter().map(|m| m.id).collect::<Vec<_>>(),
            [
                "parakeet-ultra-q8",
                "ark-asr-3b-q8",
                "qwen3-asr-1.7b-q8",
                "cohere-transcribe-2b-q8",
                "voxtral-realtime-4b-q8",
            ]
        );
        for model in &native {
            assert_eq!(model.kind, "stt");
            assert!(model.file.ends_with("-Q8_0.gguf"));
            assert_eq!((model.speed, model.accuracy), (0, 0));
            assert!(!model.recommended);
            assert!(model.template.is_empty());
        }
        assert_eq!(
            native.iter().map(|m| m.size_bytes).sum::<u64>(),
            14_356_188_000
        );
    }
}
