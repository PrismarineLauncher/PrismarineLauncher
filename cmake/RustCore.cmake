set(Launcher_RUST_CORE_ENABLED OFF CACHE INTERNAL "Whether Rust core FFI module is enabled")

find_program(CARGO_EXECUTABLE cargo)
if(NOT CARGO_EXECUTABLE)
    message(WARNING "cargo not found, Rust core FFI module will be disabled.")
    return()
endif()

set(RUST_CORE_MANIFEST "${CMAKE_SOURCE_DIR}/rust/core/Cargo.toml")
if(NOT EXISTS "${RUST_CORE_MANIFEST}")
    message(WARNING "Rust core manifest not found at ${RUST_CORE_MANIFEST}, Rust core FFI module will be disabled.")
    return()
endif()

set(RUST_CORE_TARGET_DIR "${CMAKE_BINARY_DIR}/rust-target")

if(WIN32)
    set(RUST_CORE_LIB "${RUST_CORE_TARGET_DIR}/release/rust_core.lib")
else()
    set(RUST_CORE_LIB "${RUST_CORE_TARGET_DIR}/release/librust_core.a")
endif()

file(GLOB_RECURSE RUST_CORE_SOURCES CONFIGURE_DEPENDS
    "${CMAKE_SOURCE_DIR}/rust/core/src/*.rs"
    "${CMAKE_SOURCE_DIR}/rust/core/Cargo.toml"
)

add_custom_command(
    OUTPUT "${RUST_CORE_LIB}"
    COMMAND "${CMAKE_COMMAND}" -E env "CARGO_TARGET_DIR=${RUST_CORE_TARGET_DIR}" "${CARGO_EXECUTABLE}" build --release --manifest-path "${RUST_CORE_MANIFEST}"
    DEPENDS ${RUST_CORE_SOURCES}
    COMMENT "Building Rust core static library"
    VERBATIM
)

add_custom_target(rust_core_build DEPENDS "${RUST_CORE_LIB}")

add_library(rust_core STATIC IMPORTED GLOBAL)
set_target_properties(rust_core PROPERTIES IMPORTED_LOCATION "${RUST_CORE_LIB}")
add_dependencies(rust_core rust_core_build)

set(Launcher_RUST_CORE_ENABLED ON CACHE INTERNAL "Whether Rust core FFI module is enabled")
message(STATUS "Rust core FFI module enabled")
