#include "hex_device_abi_v1.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

/* Independently authored third-party/factory device fixture: hex.pkg.soft_clip */
#define PARAM_DRIVE 1u
#define PARAM_MIX 2u
#define PARAM_TRIM 3u

typedef struct SoftClipState {
    float drive;
    float mix;
    float trim;
} SoftClipState;

static void write_str(uint8_t* dst, size_t cap, const char* src, uint32_t* out_len) {
    size_t len = strlen(src);
    if (len > cap) {
        len = cap;
    }
    memset(dst, 0, cap);
    memcpy(dst, src, len);
    *out_len = (uint32_t)len;
}

static int32_t soft_clip_describe(HexAbiDescriptor* out) {
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
    out->package_major = 1u;
    out->package_minor = 0u;
    out->package_patch = 0u;
    out->capabilities = HEX_CAP_AUDIO_EFFECT | HEX_CAP_PARAMETER_EVENTS |
                        HEX_CAP_ZERO_LATENCY | HEX_CAP_STATE_PERSISTENCE;
    out->reported_latency_frames = 0u;
    out->tail_frames = 0u;
    out->parameter_count = 3u;
    out->max_state_bytes = (uint32_t)sizeof(SoftClipState);
    write_str(out->type_id, HEX_PACKAGE_ABI_MAX_TYPE_ID_BYTES, "hex.pkg.soft_clip", &out->type_id_len);
    return HEX_ABI_OK;
}

static int32_t soft_clip_param_desc(uint32_t index, HexAbiParameterDescriptor* out) {
    if (!out || index >= 3u) {
        return HEX_ABI_INVALID_ARG;
    }
    memset(out, 0, sizeof(*out));
    out->struct_size = (uint32_t)sizeof(HexAbiParameterDescriptor);
    switch (index) {
        case 0u:
            out->id = PARAM_DRIVE;
            out->min = 1.0f;
            out->max = 12.0f;
            out->default_value = 2.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "drive", &out->name_len);
            break;
        case 1u:
            out->id = PARAM_MIX;
            out->min = 0.0f;
            out->max = 1.0f;
            out->default_value = 1.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "mix", &out->name_len);
            break;
        case 2u:
            out->id = PARAM_TRIM;
            out->min = 0.0f;
            out->max = 2.0f;
            out->default_value = 1.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "trim", &out->name_len);
            break;
        default:
            return HEX_ABI_INVALID_ARG;
    }
    return HEX_ABI_OK;
}

static int32_t soft_clip_create(uint32_t sample_rate, void** out_instance) {
    if (!out_instance || sample_rate < 8000u || sample_rate > 384000u) {
        return HEX_ABI_INVALID_RATE;
    }
    SoftClipState* st = (SoftClipState*)calloc(1, sizeof(SoftClipState));
    if (!st) {
        return HEX_ABI_ALLOCATION_FAILED;
    }
    st->drive = 2.0f;
    st->mix = 1.0f;
    st->trim = 1.0f;
    *out_instance = st;
    return HEX_ABI_OK;
}

static void soft_clip_destroy(void* instance) {
    free(instance);
}

static int32_t soft_clip_set_state(void* instance, const uint8_t* bytes, uint32_t len) {
    if (!instance || !bytes || len != sizeof(SoftClipState)) {
        return HEX_ABI_INVALID_STATE;
    }
    SoftClipState c;
    memcpy(&c, bytes, sizeof(SoftClipState));
    if (!isfinite(c.drive) || c.drive < 1.0f || c.drive > 12.0f ||
        !isfinite(c.mix) || c.mix < 0.0f || c.mix > 1.0f ||
        !isfinite(c.trim) || c.trim < 0.0f || c.trim > 2.0f) {
        return HEX_ABI_INVALID_STATE;
    }
    memcpy(instance, &c, sizeof(SoftClipState));
    return HEX_ABI_OK;
}

static int32_t soft_clip_get_state(const void* instance, uint8_t* out_bytes, uint32_t capacity, uint32_t* out_len) {
    if (!instance || !out_len) {
        return HEX_ABI_INVALID_ARG;
    }
    *out_len = (uint32_t)sizeof(SoftClipState);
    if (!out_bytes || capacity < sizeof(SoftClipState)) {
        return HEX_ABI_SIZE_MISMATCH;
    }
    memcpy(out_bytes, instance, sizeof(SoftClipState));
    return HEX_ABI_OK;
}

static int32_t soft_clip_process(
    void* instance,
    const HexAbiProcessContext* ctx,
    const float* in_stereo,
    float* out_stereo,
    const HexAbiParamEvent* param_events,
    const HexAbiNoteEvent* note_events
) {
    (void)note_events;
    if (!instance || !ctx || !in_stereo || !out_stereo) {
        return HEX_ABI_INVALID_ARG;
    }
    if (ctx->struct_size != sizeof(HexAbiProcessContext)) {
        return HEX_ABI_SIZE_MISMATCH;
    }
    if (ctx->abi_version != HEX_PACKAGE_ABI_VERSION_V1) {
        return HEX_ABI_UNSUPPORTED_ABI;
    }
    if (ctx->frames == 0u || ctx->frames > HEX_PACKAGE_ABI_MAX_FRAMES) {
        return HEX_ABI_INVALID_FRAMES;
    }
    SoftClipState* st = (SoftClipState*)instance;
    uint32_t ev_idx = 0u;
    for (uint32_t i = 0u; i < ctx->frames; ++i) {
        while (param_events && ev_idx < ctx->param_event_count && param_events[ev_idx].sample_offset <= i) {
            uint32_t id = param_events[ev_idx].parameter_id;
            float v = param_events[ev_idx].value;
            if (isfinite(v)) {
                if (id == PARAM_DRIVE && v >= 1.0f && v <= 12.0f) st->drive = v;
                else if (id == PARAM_MIX && v >= 0.0f && v <= 1.0f) st->mix = v;
                else if (id == PARAM_TRIM && v >= 0.0f && v <= 2.0f) st->trim = v;
            }
            ++ev_idx;
        }
        float dry_l = in_stereo[2u * i];
        float dry_r = in_stereo[2u * i + 1u];
        float wet_l = tanhf(dry_l * st->drive);
        float wet_r = tanhf(dry_r * st->drive);
        float m = st->mix;
        out_stereo[2u * i] = (dry_l * (1.0f - m) + wet_l * m) * st->trim;
        out_stereo[2u * i + 1u] = (dry_r * (1.0f - m) + wet_r * m) * st->trim;
    }
    return HEX_ABI_OK;
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
    out_vtable->describe = soft_clip_describe;
    out_vtable->parameter_descriptor = soft_clip_param_desc;
    out_vtable->create = soft_clip_create;
    out_vtable->destroy = soft_clip_destroy;
    out_vtable->set_state = soft_clip_set_state;
    out_vtable->get_state = soft_clip_get_state;
    out_vtable->process = soft_clip_process;
    return HEX_ABI_OK;
}
