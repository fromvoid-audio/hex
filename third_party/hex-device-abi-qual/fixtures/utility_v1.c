#include "hex_device_abi_v1.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

/* Parameter IDs */
#define PARAM_GAIN 1u
#define PARAM_BALANCE 2u
#define PARAM_WIDTH 3u
#define PARAM_INVERT_L 4u
#define PARAM_MUTE 5u

typedef struct UtilityState {
    float gain;
    float balance;
    float width;
    float invert_l;
    float mute;
} UtilityState;

static void write_str(uint8_t* dst, size_t cap, const char* src, uint32_t* out_len) {
    size_t len = strlen(src);
    if (len > cap) {
        len = cap;
    }
    memset(dst, 0, cap);
    memcpy(dst, src, len);
    *out_len = (uint32_t)len;
}

static int32_t utility_describe(HexAbiDescriptor* out) {
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
    out->parameter_count = 5u;
    out->max_state_bytes = (uint32_t)sizeof(UtilityState);
    write_str(out->type_id, HEX_PACKAGE_ABI_MAX_TYPE_ID_BYTES, "hex.pkg.utility", &out->type_id_len);
    return HEX_ABI_OK;
}

static int32_t utility_param_desc(uint32_t index, HexAbiParameterDescriptor* out) {
    if (!out || index >= 5u) {
        return HEX_ABI_INVALID_ARG;
    }
    memset(out, 0, sizeof(*out));
    out->struct_size = (uint32_t)sizeof(HexAbiParameterDescriptor);
    switch (index) {
        case 0u:
            out->id = PARAM_GAIN;
            out->min = 0.0f;
            out->max = 4.0f;
            out->default_value = 1.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "gain", &out->name_len);
            break;
        case 1u:
            out->id = PARAM_BALANCE;
            out->min = -1.0f;
            out->max = 1.0f;
            out->default_value = 0.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "balance", &out->name_len);
            break;
        case 2u:
            out->id = PARAM_WIDTH;
            out->min = 0.0f;
            out->max = 2.0f;
            out->default_value = 1.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "width", &out->name_len);
            break;
        case 3u:
            out->id = PARAM_INVERT_L;
            out->min = 0.0f;
            out->max = 1.0f;
            out->default_value = 0.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "invert_left", &out->name_len);
            break;
        case 4u:
            out->id = PARAM_MUTE;
            out->min = 0.0f;
            out->max = 1.0f;
            out->default_value = 0.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "mute", &out->name_len);
            break;
        default:
            return HEX_ABI_INVALID_ARG;
    }
    return HEX_ABI_OK;
}

static int32_t utility_create(uint32_t sample_rate, void** out_instance) {
    if (!out_instance || sample_rate < 8000u || sample_rate > 384000u) {
        return HEX_ABI_INVALID_RATE;
    }
    UtilityState* st = (UtilityState*)calloc(1, sizeof(UtilityState));
    if (!st) {
        return HEX_ABI_ALLOCATION_FAILED;
    }
    st->gain = 1.0f;
    st->balance = 0.0f;
    st->width = 1.0f;
    st->invert_l = 0.0f;
    st->mute = 0.0f;
    *out_instance = st;
    return HEX_ABI_OK;
}

static void utility_destroy(void* instance) {
    free(instance);
}

static int32_t utility_set_state(void* instance, const uint8_t* bytes, uint32_t len) {
    if (!instance || !bytes || len != sizeof(UtilityState)) {
        return HEX_ABI_INVALID_STATE;
    }
    UtilityState candidate;
    memcpy(&candidate, bytes, sizeof(UtilityState));
    if (!isfinite(candidate.gain) || candidate.gain < 0.0f || candidate.gain > 4.0f ||
        !isfinite(candidate.balance) || candidate.balance < -1.0f || candidate.balance > 1.0f ||
        !isfinite(candidate.width) || candidate.width < 0.0f || candidate.width > 2.0f ||
        !isfinite(candidate.invert_l) || !isfinite(candidate.mute)) {
        return HEX_ABI_INVALID_STATE;
    }
    memcpy(instance, &candidate, sizeof(UtilityState));
    return HEX_ABI_OK;
}

static int32_t utility_get_state(const void* instance, uint8_t* out_bytes, uint32_t capacity, uint32_t* out_len) {
    if (!instance || !out_len) {
        return HEX_ABI_INVALID_ARG;
    }
    *out_len = (uint32_t)sizeof(UtilityState);
    if (!out_bytes || capacity < sizeof(UtilityState)) {
        return HEX_ABI_SIZE_MISMATCH;
    }
    memcpy(out_bytes, instance, sizeof(UtilityState));
    return HEX_ABI_OK;
}

static void apply_param(UtilityState* st, uint32_t id, float value) {
    if (!isfinite(value)) {
        return;
    }
    switch (id) {
        case PARAM_GAIN:
            if (value >= 0.0f && value <= 4.0f) st->gain = value;
            break;
        case PARAM_BALANCE:
            if (value >= -1.0f && value <= 1.0f) st->balance = value;
            break;
        case PARAM_WIDTH:
            if (value >= 0.0f && value <= 2.0f) st->width = value;
            break;
        case PARAM_INVERT_L:
            st->invert_l = value >= 0.5f ? 1.0f : 0.0f;
            break;
        case PARAM_MUTE:
            st->mute = value >= 0.5f ? 1.0f : 0.0f;
            break;
        default:
            break;
    }
}

static int32_t utility_process(
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
    UtilityState* st = (UtilityState*)instance;
    uint32_t ev_idx = 0u;
    for (uint32_t i = 0u; i < ctx->frames; ++i) {
        while (param_events && ev_idx < ctx->param_event_count && param_events[ev_idx].sample_offset <= i) {
            apply_param(st, param_events[ev_idx].parameter_id, param_events[ev_idx].value);
            ++ev_idx;
        }
        if (st->mute >= 0.5f) {
            out_stereo[2u * i] = 0.0f;
            out_stereo[2u * i + 1u] = 0.0f;
            continue;
        }
        float l = in_stereo[2u * i];
        float r = in_stereo[2u * i + 1u];
        if (st->invert_l >= 0.5f) {
            l = -l;
        }
        float mid = 0.5f * (l + r);
        float side = 0.5f * (l - r) * st->width;
        l = mid + side;
        r = mid - side;
        float gain_l = st->gain * (st->balance > 0.0f ? (1.0f - st->balance) : 1.0f);
        float gain_r = st->gain * (st->balance < 0.0f ? (1.0f + st->balance) : 1.0f);
        out_stereo[2u * i] = l * gain_l;
        out_stereo[2u * i + 1u] = r * gain_r;
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
    out_vtable->describe = utility_describe;
    out_vtable->parameter_descriptor = utility_param_desc;
    out_vtable->create = utility_create;
    out_vtable->destroy = utility_destroy;
    out_vtable->set_state = utility_set_state;
    out_vtable->get_state = utility_get_state;
    out_vtable->process = utility_process;
    return HEX_ABI_OK;
}
