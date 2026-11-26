#include "hex_device_abi_v1.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/*
 * Static constructor canary proving that .hexdevice archive scanning and
 * installation never load or execute module initializers (#2997).
 */
__attribute__((constructor))
static void hex_ctor_canary(void) {
    const char* path = getenv("HEX_CTOR_CANARY_FILE");
    if (path && path[0] != '\0') {
        FILE* f = fopen(path, "wb");
        if (f) {
            fputs("EXECUTED", f);
            fclose(f);
        }
    }
}

static int32_t canary_describe(HexAbiDescriptor* out) {
    if (!out) return HEX_ABI_INVALID_ARG;
    memset(out, 0, sizeof(*out));
    out->struct_size = (uint32_t)sizeof(HexAbiDescriptor);
    out->magic = HEX_PACKAGE_ABI_MAGIC;
    out->abi_version = HEX_PACKAGE_ABI_VERSION_V1;
    out->device_kind = HEX_DEVICE_KIND_AUDIO_EFFECT;
    out->state_version = 1u;
    out->ui_version = 1u;
    out->package_major = 1u;
    out->capabilities = HEX_CAP_AUDIO_EFFECT | HEX_CAP_ZERO_LATENCY;
    const char* id = "hex.pkg.canary";
    memcpy(out->type_id, id, strlen(id));
    out->type_id_len = (uint32_t)strlen(id);
    return HEX_ABI_OK;
}

static int32_t canary_param_desc(uint32_t index, HexAbiParameterDescriptor* out) {
    (void)index;
    (void)out;
    return HEX_ABI_INVALID_ARG;
}

static int32_t canary_create(uint32_t sr, void** out) {
    (void)sr;
    int* p = (int*)calloc(1, sizeof(int));
    if (!p) return HEX_ABI_ALLOCATION_FAILED;
    *out = p;
    return HEX_ABI_OK;
}

static void canary_destroy(void* p) { free(p); }

static int32_t canary_set_state(void* p, const uint8_t* b, uint32_t l) {
    (void)p; (void)b; (void)l;
    return HEX_ABI_OK;
}

static int32_t canary_get_state(const void* p, uint8_t* b, uint32_t cap, uint32_t* out) {
    (void)p; (void)b; (void)cap;
    if (out) *out = 0u;
    return HEX_ABI_OK;
}

static int32_t canary_process(
    void* p, const HexAbiProcessContext* ctx,
    const float* in_s, float* out_s,
    const HexAbiParamEvent* pe, const HexAbiNoteEvent* ne
) {
    (void)p; (void)pe; (void)ne;
    if (!ctx || !in_s || !out_s) return HEX_ABI_INVALID_ARG;
    memcpy(out_s, in_s, sizeof(float) * 2u * ctx->frames);
    return HEX_ABI_OK;
}

int32_t hex_device_abi_v1_entry(uint32_t ver, HexAbiVtableV1* out) {
    if (!out || ver != HEX_PACKAGE_ABI_VERSION_V1) return HEX_ABI_UNSUPPORTED_ABI;
    if (out->struct_size != sizeof(HexAbiVtableV1)) return HEX_ABI_SIZE_MISMATCH;
    out->abi_version = HEX_PACKAGE_ABI_VERSION_V1;
    out->describe = canary_describe;
    out->parameter_descriptor = canary_param_desc;
    out->create = canary_create;
    out->destroy = canary_destroy;
    out->set_state = canary_set_state;
    out->get_state = canary_get_state;
    out->process = canary_process;
    return HEX_ABI_OK;
}
