cmake_minimum_required(VERSION 3.18)
project(lectover_native C CXX)

# qwentts.cpp names some audio-tokenizer tensors past ggml's default GGML_MAX_NAME of 64 and
# compiles itself with 128. `struct ggml_tensor` embeds `char name[GGML_MAX_NAME]`, so ggml,
# both model libraries and the Rust bindings (build.rs) must all agree: set it here, before
# any subdirectory, so it applies to every target.
add_compile_definitions(GGML_MAX_NAME=128)

# ggml first, from the llama.cpp fork (only its ggml/ subtree; no libllama). qwentts.cpp and
# qwen3-asr.cpp and reuse.cpp then skip their own ggml through their `if (NOT TARGET ggml)` guards and take
# the headers from GGML_SOURCE_DIR, so the binary carries exactly one ggml (spec D2).
# build.rs fails the build if any of the three libraries configured its own ggml anyway.
add_subdirectory(${LLAMA_DIR}/ggml ggml)
set(GGML_SOURCE_DIR ${LLAMA_DIR}/ggml CACHE PATH "the one ggml source tree" FORCE)

add_subdirectory(${QWENTTS_DIR} qwentts.cpp)
add_subdirectory(${QWEN3_ASR_DIR} qwen3-asr.cpp)
add_subdirectory(${REUSE_DIR} reuse.cpp)

foreach(t ggml qwen-core qwen3-asr reuse)
    if (NOT TARGET ${t})
        message(FATAL_ERROR "target ${t} missing after configuring ggml, qwentts.cpp, qwen3-asr.cpp and reuse.cpp")
    endif()
endforeach()

# qwentts.cpp has no install rules; qwen3-asr.cpp and ggml install their own libraries.
install(TARGETS qwen-core reuse ARCHIVE DESTINATION lib)
