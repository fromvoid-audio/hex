#include "hex_device_abi_v1.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#define PARAM_DRIVE_DB 1u
#define PARAM_TRIM_DB 2u
#define PARAM_MIX 3u
#define PARAM_CURVE 4u
#define PARAM_DC_BLOCK 5u

typedef struct SaturatorState {
    uint32_t sample_rate;
    float drive_db;
    float trim_db;
    float mix;
    float curve;
    float dc_block;
    float dc_x1_l;
    float dc_y1_l;
    float dc_x1_r;
    float dc_y1_r;
} SaturatorState;

static void write_str(uint8_t* dst, size_t cap, const char* src, uint32_t* out_len) {
    size_t len = strlen(src);
    if (len > cap) len = cap;
    memset(dst, 0, cap);
    memcpy(dst, src, len);
    *out_len = (uint32_t)len;
}

static int32_t sat_describe(HexAbiDescriptor* out) {
    if (!out) return HEX_ABI_INVALID_ARG;
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
    out->tail_frames = 64u;
    out->parameter_count = 5u;
    out->max_state_bytes = (uint32_t)sizeof(SaturatorState);
    write_str(out->type_id, HEX_PACKAGE_ABI_MAX_TYPE_ID_BYTES, "hex.pkg.saturator", &out->type_id_len);
    return HEX_ABI_OK;
}

static int32_t sat_param_desc(uint32_t index, HexAbiParameterDescriptor* out) {
    if (!out || index >= 5u) return HEX_ABI_INVALID_ARG;
    memset(out, 0, sizeof(*out));
    out->struct_size = (uint32_t)sizeof(HexAbiParameterDescriptor);
    switch (index) {
        case 0u:
            out->id = PARAM_DRIVE_DB; out->min = 0.0f; out->max = 36.0f; out->default_value = 6.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "drive", &out->name_len);
            break;
        case 1u:
            out->id = PARAM_TRIM_DB; out->min = -24.0f; out->max = 12.0f; out->default_value = 0.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "trim", &out->name_len);
            break;
        case 2u:
            out->id = PARAM_MIX; out->min = 0.0f; out->max = 1.0f; out->default_value = 1.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "mix", &out->name_len);
            break;
        case 3u:
            out->id = PARAM_CURVE; out->min = 0.0f; out->max = 2.0f; out->default_value = 0.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "curve", &out->name_len);
            break;
        case 4u:
            out->id = PARAM_DC_BLOCK; out->min = 0.0f; out->max = 1.0f; out->default_value = 1.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "dc_block", &out->name_len);
            break;
        default:
            return HEX_ABI_INVALID_ARG;
    }
    return HEX_ABI_OK;
}

static int32_t sat_create(uint32_t sample_rate, void** out_instance) {
    if (!out_instance || sample_rate < 8000u || sample_rate > 384000u) return HEX_ABI_INVALID_RATE;
    SaturatorState* st = (SaturatorState*)calloc(1, sizeof(SaturatorState));
    if (!st) return HEX_ABI_ALLOCATION_FAILED;
    st->sample_rate = sample_rate;
    st->drive_db = 6.0f;
    st->trim_db = 0.0f;
    st->mix = 1.0f;
    st->curve = 0.0f;
    st->dc_block = 1.0f;
    *out_instance = st;
    return HEX_ABI_OK;
}

static void sat_destroy(void* instance) { free(instance); }

static int32_t sat_set_state(void* instance, const uint8_t* bytes, uint32_t len) {
    if (!instance || !bytes || len != sizeof(SaturatorState)) return HEX_ABI_INVALID_STATE;
    SaturatorState cand;
    memcpy(&cand, bytes, sizeof(SaturatorState));
    if (!isfinite(cand.drive_db) || cand.drive_db < 0.0f || cand.drive_db > 36.0f ||
        !isfinite(cand.trim_db) || cand.trim_db < -24.0f || cand.trim_db > 12.0f ||
        !isfinite(cand.mix) || cand.mix < 0.0f || cand.mix > 1.0f ||
        !isfinite(cand.curve) || cand.curve < 0.0f || cand.curve > 2.0f ||
        !isfinite(cand.dc_block) || !isfinite(cand.dc_x1_l) || !isfinite(cand.dc_y1_l) ||
        !isfinite(cand.dc_x1_r) || !isfinite(cand.dc_y1_r)) {
        return HEX_ABI_INVALID_STATE;
    }
    uint32_t sr = ((SaturatorState*)instance)->sample_rate;
    memcpy(instance, &cand, sizeof(SaturatorState));
    ((SaturatorState*)instance)->sample_rate = sr;
    return HEX_ABI_OK;
}

static int32_t sat_get_state(const void* instance, uint8_t* out_bytes, uint32_t capacity, uint32_t* out_len) {
    if (!instance || !out_len) return HEX_ABI_INVALID_ARG;
    *out_len = (uint32_t)sizeof(SaturatorState);
    if (!out_bytes || capacity < sizeof(SaturatorState)) return HEX_ABI_SIZE_MISMATCH;
    memcpy(out_bytes, instance, sizeof(SaturatorState));
    return HEX_ABI_OK;
}

static float shape_sample(float x, float curve) {
    if (curve < 0.5f) {
        return tanhf(x);
    } else if (curve < 1.5f) {
        return x > 1.0f ? 1.0f : (x < -1.0f ? -1.0f : x);
    } else {
        float c = x > 1.5f ? 1.5f : (x < -1.5f ? -1.5f : x);
        return c - (c * c * c) / 6.75f;
    }
}

static int32_t sat_process(
    void* instance,
    const HexAbiProcessContext* ctx,
    const float* in_stereo,
    float* out_stereo,
    const HexAbiParamEvent* param_events,
    const HexAbiNoteEvent* note_events
) {
    (void)note_events;
    if (!instance || !ctx || !in_stereo || !out_stereo) return HEX_ABI_INVALID_ARG;
    if (ctx->struct_size != sizeof(HexAbiProcessContext)) return HEX_ABI_SIZE_MISMATCH;
    if (ctx->abi_version != HEX_PACKAGE_ABI_VERSION_V1) return HEX_ABI_UNSUPPORTED_ABI;
    if (ctx->frames == 0u || ctx->frames > HEX_PACKAGE_ABI_MAX_FRAMES) return HEX_ABI_INVALID_FRAMES;

    SaturatorState* st = (SaturatorState*)instance;
    const float dc_r = 0.995f;
    uint32_t ev_idx = 0u;
    for (uint32_t i = 0u; i < ctx->frames; ++i) {
        while (param_events && ev_idx < ctx->param_event_count && param_events[ev_idx].sample_offset <= i) {
            uint32_t id = param_events[ev_idx].parameter_id;
            float v = param_events[ev_idx].value;
            if (isfinite(v)) {
                if (id == PARAM_DRIVE_DB && v >= 0.0f && v <= 36.0f) st->drive_db = v;
                else if (id == PARAM_TRIM_DB && v >= -24.0f && v <= 12.0f) st->trim_db = v;
                else if (id == PARAM_MIX && v >= 0.0f && v <= 1.0f) st->mix = v;
                else if (id == PARAM_CURVE && v >= 0.0f && v <= 2.0f) st->curve = v;
                else if (id == PARAM_DC_BLOCK) st->dc_block = v >= 0.5f ? 1.0f : 0.0f;
            }
            ++ev_idx;
        }
        float drive = powf(10.0f, st->drive_db * 0.05f);
        float trim = powf(10.0f, st->trim_db * 0.05f);
        float dry_l = in_stereo[2u * i];
        float dry_r = in_stereo[2u * i + 1u];
        float wet_l = shape_sample(dry_l * drive, st->curve) * trim;
        float wet_r = shape_sample(dry_r * drive, st->curve) * trim;
        if (st->dc_block >= 0.5f) {
            float y_l = wet_l - st->dc_x1_l + dc_r * st->dc_y1_l;
            st->dc_x1_l = wet_l;
            st->dc_y1_l = y_l;
            wet_l = y_l;
            float y_r = wet_r - st->dc_x1_r + dc_r * st->dc_y1_r;
            st->dc_x1_r = wet_r;
            st->dc_y1_r = y_r;
            wet_r = y_r;
        }
        out_stereo[2u * i] = dry_l + (wet_l - dry_l) * st->mix;
        out_stereo[2u * i + 1u] = dry_r + (wet_r - dry_r) * st->mix;
    }
    return HEX_ABI_OK;
}

int32_t hex_device_abi_v1_entry(uint32_t requested_abi_version, HexAbiVtableV1* out_vtable) {
    if (!out_vtable) return HEX_ABI_INVALID_ARG;
    if (requested_abi_version != HEX_PACKAGE_ABI_VERSION_V1) return HEX_ABI_UNSUPPORTED_ABI;
    if (out_vtable->struct_size != sizeof(HexAbiVtableV1)) return HEX_ABI_SIZE_MISMATCH;
    out_vtable->abi_version = HEX_PACKAGE_ABI_VERSION_V1;
    out_vtable->describe = sat_describe;
    out_vtable->parameter_descriptor = sat_param_desc;
    out_vtable->create = sat_create;
    out_vtable->destroy = sat_destroy;
    out_vtable->set_state = sat_set_state;
    out_vtable->get_state = sat_get_state;
    out_vtable->process = sat_process;
    return HEX_ABI_OK;
}
