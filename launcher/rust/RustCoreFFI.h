#pragma once

#include <cstddef>
#include <cstdint>

extern "C" {

struct PrismarineTimestampResult {
    int64_t unix_ms_utc;
    int32_t offset_seconds;
    uint8_t is_valid;
};

int32_t prismarine_parse_s3_time(const char* input, PrismarineTimestampResult* out_result);
int32_t prismarine_format_s3_time(int64_t unix_ms_utc, int32_t offset_seconds, char* out_buffer, size_t out_buffer_len);

}
