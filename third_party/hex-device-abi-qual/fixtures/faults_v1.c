#include "hex_device_abi_v1.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/*
 * FAULT_MODE values compiled independently via -DFAULT_MODE=N:
 *   1 = malformed descriptor magic
 *   2 = malformed descriptor struct_size
 *   3 = unsupported future ABI version
 *   4 = emits NaN during process
 *   5 = crashes (abort) during process
 *   6 = hangs indefinitely during process
 *   7 = hangs indefinitely during destroy/teardown
 */
#ifndef FAULT_MODE
#define FAULT_MODE 4
#endif

static int32_t fault_describe(HexAbiDescriptor* out) {
    if (!out) {
        return HEX_ABI_INVALID_ARG;
    }
    memset(out, 0, sizeof(*out));
    out->struct_size = (uint32_t)sizeof(HexAbiDescriptor);
    out->magic = HEX_PACKAGE_ABI_MAGIC;
    out->abi_version = HEX_PACKAGE_ABI_VERSION_V1;
    out->device_kind = HEX_DEVICE_KIND_AUDIO_EFFECT;
    out->state_version = 1u;
    out->ui_version = 1u;
    out->package_major = 0u;
    out->package_minor = 1u;
    out->package_patch = 0u;
    out->capabilities = HEX_CAP_AUDIO_EFFECT | HEX_CAP_ZERO_LATENCY;
    const char* name = "hex.pkg.fault";
    size_t len = strlen(name);
    memcpy(out->type_id, name, len);
    out->type_id_len = (uint32_t)len;

#if FAULT_MODE == 1
    out->magic = 0xBAD0BEEFu;
#elif FAULT_MODE == 2
    out->struct_size = 16u;
#elif FAULT_MODE == 3
    out->abi_version = 99u;
#endif
    return HEX_ABI_OK;
}

static int32_t fault_param_desc(uint32_t index, HexAbiParameterDescriptor* out) {
    (void)index;
    (void)out;
    return HEX_ABI_INVALID_ARG;
}

static int32_t fault_create(uint32_t sample_rate, void** out_instance) {
    (void)sample_rate;
    int* token = (int*)malloc(sizeof(int));
    if (!token) {
        return HEX_ABI_ALLOCATION_FAILED;
    }
    *token = FAULT_MODE;
    *out_instance = token;
    return HEX_ABI_OK;
}

static void fault_destroy(void* instance) {
#if FAULT_MODE == 7
    for (;;) {
        sleep(10);
    }
#endif
    free(instance);
}

static int32_t fault_set_state(void* instance, const uint8_t* bytes, uint32_t len) {
    (void)instance;
    (void)bytes;
    (void)len;
    return HEX_ABI_OK;
}

static int32_t fault_get_state(const void* instance, uint8_t* out_bytes, uint32_t capacity, uint32_t* out_len) {
    (void)instance;
    (void)out_bytes;
    (void)capacity;
    if (out_len) {
        *out_len = 0u;
    }
    return HEX_ABI_OK;
}

static int32_t fault_process(
    void* instance,
    const HexAbiProcessContext* ctx,
    const float* in_stereo,
    float* out_stereo,
    const HexAbiParamEvent* param_events,
    const HexAbiNoteEvent* note_events
) {
    (void)instance;
    (void)in_stereo;
    (void)param_events;
    (void)note_events;
    if (!ctx || !out_stereo) {
        return HEX_ABI_INVALID_ARG;
    }
#if FAULT_MODE == 4
    for (uint32_t i = 0u; i < ctx->frames; ++i) {
        out_stereo[2u * i] = NAN;
        out_stereo[2u * i + 1u] = INFINITY;
    }
    return HEX_ABI_OK;
#elif FAULT_MODE == 5
    abort();
#elif FAULT_MODE == 6
    for (;;) {
        sleep(10);
    }
#else
    memset(out_stereo, 0, sizeof(float) * 2u * ctx->frames);
    return HEX_ABI_OK;
#endif
}

int32_t hex_device_abi_v1_entry(uint32_t requested_abi_version, HexAbiVtableV1* out_vtable) {
    if (!out_vtable) {
        return HEX_ABI_INVALID_ARG;
    }
    if (requested_abi_version != HEX_PACKAGE_ABI_VERSION_V1) {
        return HEX_ABI_UNSUPPORTED_ABI;
    }
    if (out_vtable->struct_size != sizeof(HexAbiVtableV1)) {
        return HEX_ABI_SIZE_MISMATCH;
    }
    out_vtable->abi_version = HEX_PACKAGE_ABI_VERSION_V1;
    out_vtable->describe = fault_describe;
    out_vtable->parameter_descriptor = fault_param_desc;
    out_vtable->create = fault_create;
    out_vtable->destroy = fault_destroy;
    out_vtable->set_state = fault_set_state;
    out_vtable->get_state = fault_get_state;
    out_vtable->process = fault_process;
    return HEX_ABI_OK;
}
