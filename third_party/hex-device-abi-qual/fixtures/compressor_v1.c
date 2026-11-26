#include "hex_device_abi_v1.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#define PARAM_THRESHOLD 1u
#define PARAM_RATIO 2u
#define PARAM_ATTACK_MS 3u
#define PARAM_RELEASE_MS 4u
#define PARAM_MAKEUP_DB 5u
#define PARAM_MIX 6u

typedef struct CompressorState {
    uint32_t sample_rate;
    float threshold_db;
    float ratio;
    float attack_ms;
    float release_ms;
    float makeup_db;
    float mix;
    float env;
} CompressorState;

static void write_str(uint8_t* dst, size_t cap, const char* src, uint32_t* out_len) {
    size_t len = strlen(src);
    if (len > cap) len = cap;
    memset(dst, 0, cap);
    memcpy(dst, src, len);
    *out_len = (uint32_t)len;
}

static int32_t comp_describe(HexAbiDescriptor* out) {
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
    out->tail_frames = 0u;
    out->parameter_count = 6u;
    out->max_state_bytes = (uint32_t)sizeof(CompressorState);
    write_str(out->type_id, HEX_PACKAGE_ABI_MAX_TYPE_ID_BYTES, "hex.pkg.compressor", &out->type_id_len);
    return HEX_ABI_OK;
}

static int32_t comp_param_desc(uint32_t index, HexAbiParameterDescriptor* out) {
    if (!out || index >= 6u) return HEX_ABI_INVALID_ARG;
    memset(out, 0, sizeof(*out));
    out->struct_size = (uint32_t)sizeof(HexAbiParameterDescriptor);
    switch (index) {
        case 0u:
            out->id = PARAM_THRESHOLD; out->min = -60.0f; out->max = 0.0f; out->default_value = -12.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "threshold", &out->name_len);
            break;
        case 1u:
            out->id = PARAM_RATIO; out->min = 1.0f; out->max = 20.0f; out->default_value = 4.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "ratio", &out->name_len);
            break;
        case 2u:
            out->id = PARAM_ATTACK_MS; out->min = 0.1f; out->max = 200.0f; out->default_value = 10.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "attack", &out->name_len);
            break;
        case 3u:
            out->id = PARAM_RELEASE_MS; out->min = 5.0f; out->max = 1000.0f; out->default_value = 100.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "release", &out->name_len);
            break;
        case 4u:
            out->id = PARAM_MAKEUP_DB; out->min = -12.0f; out->max = 24.0f; out->default_value = 0.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "gain", &out->name_len);
            break;
        case 5u:
            out->id = PARAM_MIX; out->min = 0.0f; out->max = 1.0f; out->default_value = 1.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "mix", &out->name_len);
            break;
        default:
            return HEX_ABI_INVALID_ARG;
    }
    return HEX_ABI_OK;
}

static int32_t comp_create(uint32_t sample_rate, void** out_instance) {
    if (!out_instance || sample_rate < 8000u || sample_rate > 384000u) return HEX_ABI_INVALID_RATE;
    CompressorState* st = (CompressorState*)calloc(1, sizeof(CompressorState));
    if (!st) return HEX_ABI_ALLOCATION_FAILED;
    st->sample_rate = sample_rate;
    st->threshold_db = -12.0f;
    st->ratio = 4.0f;
    st->attack_ms = 10.0f;
    st->release_ms = 100.0f;
    st->makeup_db = 0.0f;
    st->mix = 1.0f;
    st->env = 0.0f;
    *out_instance = st;
    return HEX_ABI_OK;
}

static void comp_destroy(void* instance) { free(instance); }

static int32_t comp_set_state(void* instance, const uint8_t* bytes, uint32_t len) {
    if (!instance || !bytes || len != sizeof(CompressorState)) return HEX_ABI_INVALID_STATE;
    CompressorState cand;
    memcpy(&cand, bytes, sizeof(CompressorState));
    if (!isfinite(cand.threshold_db) || cand.threshold_db < -60.0f || cand.threshold_db > 0.0f ||
        !isfinite(cand.ratio) || cand.ratio < 1.0f || cand.ratio > 20.0f ||
        !isfinite(cand.attack_ms) || cand.attack_ms < 0.1f || cand.attack_ms > 200.0f ||
        !isfinite(cand.release_ms) || cand.release_ms < 5.0f || cand.release_ms > 1000.0f ||
        !isfinite(cand.makeup_db) || cand.makeup_db < -12.0f || cand.makeup_db > 24.0f ||
        !isfinite(cand.mix) || cand.mix < 0.0f || cand.mix > 1.0f ||
        !isfinite(cand.env) || cand.env < 0.0f) {
        return HEX_ABI_INVALID_STATE;
    }
    uint32_t sr = ((CompressorState*)instance)->sample_rate;
    memcpy(instance, &cand, sizeof(CompressorState));
    ((CompressorState*)instance)->sample_rate = sr;
    return HEX_ABI_OK;
}

static int32_t comp_get_state(const void* instance, uint8_t* out_bytes, uint32_t capacity, uint32_t* out_len) {
    if (!instance || !out_len) return HEX_ABI_INVALID_ARG;
    *out_len = (uint32_t)sizeof(CompressorState);
    if (!out_bytes || capacity < sizeof(CompressorState)) return HEX_ABI_SIZE_MISMATCH;
    memcpy(out_bytes, instance, sizeof(CompressorState));
    return HEX_ABI_OK;
}

static int32_t comp_process(
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

    CompressorState* st = (CompressorState*)instance;
    float sr = (float)st->sample_rate;
    uint32_t ev_idx = 0u;
    for (uint32_t i = 0u; i < ctx->frames; ++i) {
        while (param_events && ev_idx < ctx->param_event_count && param_events[ev_idx].sample_offset <= i) {
            uint32_t id = param_events[ev_idx].parameter_id;
            float v = param_events[ev_idx].value;
            if (isfinite(v)) {
                if (id == PARAM_THRESHOLD && v >= -60.0f && v <= 0.0f) st->threshold_db = v;
                else if (id == PARAM_RATIO && v >= 1.0f && v <= 20.0f) st->ratio = v;
                else if (id == PARAM_ATTACK_MS && v >= 0.1f && v <= 200.0f) st->attack_ms = v;
                else if (id == PARAM_RELEASE_MS && v >= 5.0f && v <= 1000.0f) st->release_ms = v;
                else if (id == PARAM_MAKEUP_DB && v >= -12.0f && v <= 24.0f) st->makeup_db = v;
                else if (id == PARAM_MIX && v >= 0.0f && v <= 1.0f) st->mix = v;
            }
            ++ev_idx;
        }
        float dry_l = in_stereo[2u * i];
        float dry_r = in_stereo[2u * i + 1u];
        float peak = fmaxf(fabsf(dry_l), fabsf(dry_r));
        float atk = expf(-1.0f / fmaxf(1.0f, st->attack_ms * 0.001f * sr));
        float rel = expf(-1.0f / fmaxf(1.0f, st->release_ms * 0.001f * sr));
        float coeff = peak > st->env ? atk : rel;
        st->env = coeff * st->env + (1.0f - coeff) * peak;

        float gr_db = 0.0f;
        if (st->env > 1e-6f) {
            float env_db = 20.0f * log10f(st->env);
            if (env_db > st->threshold_db) {
                gr_db = (st->threshold_db - env_db) * (1.0f - 1.0f / st->ratio);
            }
        }
        float gain = powf(10.0f, (gr_db + st->makeup_db) * 0.05f);
        float wet_l = dry_l * gain;
        float wet_r = dry_r * gain;
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
    out_vtable->describe = comp_describe;
    out_vtable->parameter_descriptor = comp_param_desc;
    out_vtable->create = comp_create;
    out_vtable->destroy = comp_destroy;
    out_vtable->set_state = comp_set_state;
    out_vtable->get_state = comp_get_state;
    out_vtable->process = comp_process;
    return HEX_ABI_OK;
}
