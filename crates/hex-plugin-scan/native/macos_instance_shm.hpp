// Bounded shared-memory SPSC ring buffer for per-quantum VST3 PCM transport.
// Must match third_party/hex-shm-ring/src/raw.rs byte-for-byte.
#pragma once
#include <atomic>
#include <cerrno>
#include <cstdint>
#include <cstring>
#include <poll.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <unistd.h>

namespace hex_instance_shm {
constexpr uint8_t DOORBELL_BYTE = 0x01;
constexpr uint8_t SCM_HANDSHAKE_BYTE = 'S';
constexpr uint32_t VERSION = 1;
constexpr size_t SLOT_COUNT = 4;
constexpr size_t SLOT_CAPACITY = 16384;
constexpr size_t HEADER_BYTES = 4096;
constexpr size_t REQ_SLOTS_OFFSET = HEADER_BYTES;
constexpr size_t REP_SLOTS_OFFSET = HEADER_BYTES + SLOT_COUNT * SLOT_CAPACITY;
constexpr size_t REGION_BYTES = HEADER_BYTES + 2 * SLOT_COUNT * SLOT_CAPACITY;

static_assert(REGION_BYTES == 135168, "shared-memory layout size mismatch");
static_assert(std::atomic<uint64_t>::is_always_lock_free, "atomic u64 must be lock-free");
static_assert(std::atomic<uint32_t>::is_always_lock_free, "atomic u32 must be lock-free");

// Header field byte offsets (aligned to 64-byte cache lines).
constexpr size_t OFF_MAGIC = 0;
constexpr size_t OFF_VERSION = 8;
constexpr size_t OFF_SLOT_COUNT = 12;
constexpr size_t OFF_SLOT_CAPACITY = 16;
constexpr size_t OFF_RESERVED = 20;

constexpr size_t OFF_REQ_WRITE_SEQ = 64;
constexpr size_t OFF_REQ_READ_SEQ = 72;
constexpr size_t OFF_REQ_SLOT_LEN = 80;

constexpr size_t OFF_REP_WRITE_SEQ = 128;
constexpr size_t OFF_REP_READ_SEQ = 136;
constexpr size_t OFF_REP_SLOT_LEN = 144;

constexpr size_t OFF_FAULT_FLAG = 192;

class ShmRing {
    uint8_t* base_ = nullptr;

    std::atomic<uint32_t>* atomic_u32(size_t offset) const {
        return reinterpret_cast<std::atomic<uint32_t>*>(base_ + offset);
    }
    std::atomic<uint64_t>* atomic_u64(size_t offset) const {
        return reinterpret_cast<std::atomic<uint64_t>*>(base_ + offset);
    }
    std::atomic<uint64_t>* req_write_seq() const { return atomic_u64(OFF_REQ_WRITE_SEQ); }
    std::atomic<uint64_t>* req_read_seq() const { return atomic_u64(OFF_REQ_READ_SEQ); }
    std::atomic<uint32_t>* req_slot_len(size_t slot) const { return atomic_u32(OFF_REQ_SLOT_LEN + slot * 4); }
    std::atomic<uint64_t>* rep_write_seq() const { return atomic_u64(OFF_REP_WRITE_SEQ); }
    std::atomic<uint64_t>* rep_read_seq() const { return atomic_u64(OFF_REP_READ_SEQ); }
    std::atomic<uint32_t>* rep_slot_len(size_t slot) const { return atomic_u32(OFF_REP_SLOT_LEN + slot * 4); }

    bool validate_header() const {
        return std::memcmp(base_ + OFF_MAGIC, "HXSHM001", 8) == 0 &&
               atomic_u32(OFF_VERSION)->load(std::memory_order_acquire) == VERSION &&
               atomic_u32(OFF_SLOT_COUNT)->load(std::memory_order_acquire) == SLOT_COUNT &&
               atomic_u32(OFF_SLOT_CAPACITY)->load(std::memory_order_acquire) == SLOT_CAPACITY &&
               atomic_u32(OFF_RESERVED)->load(std::memory_order_acquire) == 0;
    }

public:
    ShmRing() = default;
    ShmRing(const ShmRing&) = delete;
    ShmRing& operator=(const ShmRing&) = delete;
    ~ShmRing() { close(); }

    bool active() const { return base_ != nullptr; }

    void close() {
        if (base_) {
            ::munmap(base_, REGION_BYTES);
            base_ = nullptr;
        }
    }

    // Receive the host-allocated anonymous POSIX shared-memory descriptor via
    // SCM_RIGHTS, validate its size/header, mmap it, and close the fd immediately.
    bool recv_and_map(int socket_fd) {
        close();
        uint8_t dummy = 0;
        iovec iov{&dummy, sizeof(dummy)};
        alignas(cmsghdr) uint8_t cmsg_buf[CMSG_SPACE(sizeof(int))]{};
        msghdr msg{};
        msg.msg_iov = &iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cmsg_buf;
        msg.msg_controllen = sizeof(cmsg_buf);

        ssize_t received = -1;
        while (true) {
            received = ::recvmsg(socket_fd, &msg, 0);
            if (received < 0 && errno == EINTR) continue;
            if (received < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
                pollfd pfd{socket_fd, POLLIN, 0};
                const int ready = ::poll(&pfd, 1, 10);
                if (ready < 0 && errno != EINTR) return false;
                if (ready > 0 && (pfd.revents & (POLLERR | POLLNVAL))) return false;
                continue;
            }
            break;
        }
        if (received != 1) return false;

        int shm_fd = -1;
        bool valid_cmsg = false;
        for (cmsghdr* cmsg = CMSG_FIRSTHDR(&msg); cmsg != nullptr; cmsg = CMSG_NXTHDR(&msg, cmsg)) {
            if (cmsg->cmsg_level == SOL_SOCKET && cmsg->cmsg_type == SCM_RIGHTS) {
                const auto* data = CMSG_DATA(cmsg);
                const size_t data_len = static_cast<size_t>(cmsg->cmsg_len) -
                                        static_cast<size_t>(data - reinterpret_cast<const uint8_t*>(cmsg));
                const size_t fd_count = data_len / sizeof(int);
                for (size_t i = 0; i < fd_count; ++i) {
                    int candidate = -1;
                    std::memcpy(&candidate, data + i * sizeof(int), sizeof(int));
                    if (candidate >= 0) {
                        if (shm_fd < 0 && fd_count == 1) {
                            shm_fd = candidate;
                            valid_cmsg = true;
                        } else {
                            ::close(candidate);
                            valid_cmsg = false;
                        }
                    }
                }
            }
        }
        if ((msg.msg_flags & MSG_CTRUNC) || dummy != SCM_HANDSHAKE_BYTE || !valid_cmsg || shm_fd < 0) {
            if (shm_fd >= 0) ::close(shm_fd);
            return false;
        }

        struct stat st{};
        if (::fstat(shm_fd, &st) != 0 || st.st_size < static_cast<off_t>(REGION_BYTES)) {
            ::close(shm_fd);
            return false;
        }
        void* mapped = ::mmap(nullptr, REGION_BYTES, PROT_READ | PROT_WRITE, MAP_SHARED, shm_fd, 0);
        ::close(shm_fd);
        if (mapped == MAP_FAILED || !mapped) return false;

        base_ = static_cast<uint8_t*>(mapped);
        if (!validate_header()) {
            close();
            return false;
        }
        return true;
    }

    bool read_request(uint8_t* dst, size_t dst_cap, size_t& out_len) const {
        if (!base_) return false;
        const uint64_t r = req_read_seq()->load(std::memory_order_relaxed);
        const uint64_t w = req_write_seq()->load(std::memory_order_acquire);
        if (w <= r || (w - r) > SLOT_COUNT) return false;
        const size_t slot = static_cast<size_t>(r % SLOT_COUNT);
        const uint32_t len = req_slot_len(slot)->load(std::memory_order_relaxed);
        if (len == 0 || len > SLOT_CAPACITY || len > dst_cap) return false;
        std::memcpy(dst, base_ + REQ_SLOTS_OFFSET + slot * SLOT_CAPACITY, len);
        req_read_seq()->store(r + 1, std::memory_order_release);
        out_len = len;
        return true;
    }

    bool write_reply(const uint8_t* src, size_t len) const {
        if (!base_ || len == 0 || len > SLOT_CAPACITY) return false;
        const uint64_t w = rep_write_seq()->load(std::memory_order_relaxed);
        const uint64_t r = rep_read_seq()->load(std::memory_order_acquire);
        if (w < r || (w - r) >= SLOT_COUNT) return false;
        const size_t slot = static_cast<size_t>(w % SLOT_COUNT);
        std::memcpy(base_ + REP_SLOTS_OFFSET + slot * SLOT_CAPACITY, src, len);
        rep_slot_len(slot)->store(static_cast<uint32_t>(len), std::memory_order_relaxed);
        rep_write_seq()->store(w + 1, std::memory_order_release);
        return true;
    }

    bool is_faulted() const {
        return !base_ || atomic_u32(OFF_FAULT_FLAG)->load(std::memory_order_acquire) != 0;
    }
};
} // namespace hex_instance_shm
