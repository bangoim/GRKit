#ifndef GRKIT_CONTROL_MAILBOX_H
#define GRKIT_CONTROL_MAILBOX_H
/* Internal single-producer/single-consumer transport, not a persistence or
   hot-loading implementation. The producer is the Linux service. The consumer
   is the App task. Neither side writes a slot owned by the other side.
   Map this buffer coherently before use: a CPU barrier does not flush aliases. */
#include "abi.h"
#define CONTROL_MAGIC GRKIT_CONTROL_MAGIC_VALUE
#define CONTROL_VERSION GRKIT_CONTROL_VERSION_VALUE
#define CONTROL_PAYLOAD_BYTES GRKIT_CONTROL_PAYLOAD_BYTES
#define CONTROL_EMPTY GRKIT_CONTROL_EMPTY
#define CONTROL_ACCEPTED GRKIT_CONTROL_ACCEPTED
#define CONTROL_BAD_CONTEXT GRKIT_CONTROL_BAD_CONTEXT
#define CONTROL_BAD_LENGTH GRKIT_CONTROL_BAD_LENGTH
#define CONTROL_BAD_DIGEST GRKIT_CONTROL_BAD_DIGEST
#define CONTROL_CHANGED GRKIT_CONTROL_CHANGED
#define CONTROL_UNSUPPORTED GRKIT_CONTROL_UNSUPPORTED
#define CONTROL_INVALID GRKIT_CONTROL_INVALID
#define CONTROL_BUSY GRKIT_CONTROL_BUSY
#define CONTROL_ATTACH GRKIT_CONTROL_ATTACH
#define CONTROL_PING GRKIT_CONTROL_PING
#define CONTROL_SEQUENCE_MAX GRKIT_CONTROL_SEQUENCE_MAX
#ifndef CONTROL_BARRIER
#define CONTROL_BARRIER() __asm__ volatile("dmb ish" ::: "memory")
#endif

/* ARMv7 PoC maintenance is restricted to cache lines owned by one side.
   The mailbox base and every ownership boundary are 64-byte aligned. This
   also accommodates the 32-byte data cache lines on supported Cortex-A cores.
   Linux owns producer/payload; App owns descriptor/consumer. */
#ifndef CONTROL_INVALIDATE
#ifdef __arm__
static void control_invalidate(volatile void *address,u32 length) {
    u32 at=(u32)address,end=at+length;
    __asm__ volatile("dsb sy" ::: "memory");
    for(;at<end;at+=32) __asm__ volatile("mcr p15, 0, %0, c7, c6, 1" :: "r"(at) : "memory");
    __asm__ volatile("dsb sy" ::: "memory");
}
#define CONTROL_INVALIDATE(address,length) control_invalidate(address,length)
#else
#define CONTROL_INVALIDATE(address,length) ((void)(address),(void)(length))
#endif
#endif
#ifndef CONTROL_CLEAN
#ifdef __arm__
static void control_clean(volatile void *address,u32 length) {
    u32 at=(u32)address,end=at+length;
    __asm__ volatile("dsb sy" ::: "memory");
    for(;at<end;at+=32) __asm__ volatile("mcr p15, 0, %0, c7, c10, 1" :: "r"(at) : "memory");
    __asm__ volatile("dsb sy" ::: "memory");
}
#define CONTROL_CLEAN(address,length) control_clean(address,length)
#else
#define CONTROL_CLEAN(address,length) ((void)(address),(void)(length))
#endif
#endif

struct control_mailbox {
    /* Runtime-owned descriptor. ATTACH installs epoch once per buffer. */
    u32 magic, version, capacity, epoch[4], owner_entry, owner_state, owner_poll, descriptor_padding[6];
    /* Producer-owned request and release markers. Never wrap request IDs. */
    u32 request, release, cancel;
    u32 request_epoch[4], command, expected_generation, length, checksum;
    u32 producer_padding[5];
    /* Consumer-owned result. READY publishes all these result fields. */
    u32 claimed, ready, result, generation, response_length, response_checksum, consumer_padding[10];
    u8 payload[CONTROL_PAYLOAD_BYTES];
    /* Consumer-owned bytes, immutable until producer RELEASE. */
    u8 response[CONTROL_PAYLOAD_BYTES];
};
/* Validate the native layout against the generated Rust/Linux ABI. */
_Static_assert(__builtin_offsetof(struct control_mailbox,magic)==GRKIT_CONTROL_MAGIC,"control magic offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,version)==GRKIT_CONTROL_VERSION,"control version offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,capacity)==GRKIT_CONTROL_CAPACITY,"control capacity offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,epoch)==GRKIT_CONTROL_EPOCH,"control epoch offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,request)==GRKIT_CONTROL_REQUEST,"control request offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,release)==GRKIT_CONTROL_RELEASE,"control release offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,cancel)==GRKIT_CONTROL_CANCEL,"control cancel offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,request_epoch)==GRKIT_CONTROL_REQUEST_EPOCH,"control request_epoch offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,command)==GRKIT_CONTROL_COMMAND,"control command offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,expected_generation)==GRKIT_CONTROL_EXPECTED_GENERATION,"control expected_generation offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,length)==GRKIT_CONTROL_LENGTH,"control length offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,checksum)==GRKIT_CONTROL_CHECKSUM,"control checksum offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,claimed)==GRKIT_CONTROL_CLAIMED,"control claimed offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,ready)==GRKIT_CONTROL_READY,"control ready offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,result)==GRKIT_CONTROL_RESULT,"control result offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,generation)==GRKIT_CONTROL_GENERATION,"control generation offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,payload)==GRKIT_CONTROL_PAYLOAD,"control payload offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,owner_entry)==GRKIT_CONTROL_OWNER_ENTRY,"control owner entry offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,owner_poll)==GRKIT_CONTROL_OWNER_POLL,"control owner poll offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,owner_state)==GRKIT_CONTROL_OWNER_STATE,"control owner state offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,response_length)==GRKIT_CONTROL_RESPONSE_LENGTH,"control response length offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,response_checksum)==GRKIT_CONTROL_RESPONSE_CHECKSUM,"control response checksum offset");
_Static_assert(__builtin_offsetof(struct control_mailbox,response)==GRKIT_CONTROL_RESPONSE,"control response offset");
_Static_assert(sizeof(struct control_mailbox)==GRKIT_CONTROL_SIZE,"control size");
struct control_request {
    u32 sequence, command, expected_generation, length;
    u8 payload[CONTROL_PAYLOAD_BYTES];
};

_Static_assert(sizeof(struct control_request)==GRKIT_CONTROL_REQUEST_SIZE,"control request size");

static u32 control_checksum(const u8 *bytes,u32 length) {
    u32 crc=~0U, i, bit;
    for(i=0;i<length;++i) {
        crc^=bytes[i];
        for(bit=0;bit<8;++bit) crc=(crc>>1)^(0xedb88320U & (0U-(crc&1U)));
    }
    return ~crc;
}
static void control_init(volatile struct control_mailbox *box,const u32 epoch[4],u32 generation) {
    volatile u8 *bytes=(volatile u8 *)box;
    u32 i;
    for(i=0;i<sizeof(*box);++i) bytes[i]=0;
    box->version=CONTROL_VERSION;
    box->capacity=CONTROL_PAYLOAD_BYTES;
    for(i=0;i<4;++i) box->epoch[i]=epoch[i];
    box->generation=generation;
    CONTROL_BARRIER();
    box->magic=CONTROL_MAGIC;
}
static u32 control_context(volatile struct control_mailbox *box,const u32 epoch[4]) {
    u32 i;
    if(box->magic!=CONTROL_MAGIC) return 0;
    CONTROL_BARRIER();
    if(box->version!=CONTROL_VERSION || box->capacity!=CONTROL_PAYLOAD_BYTES) return 0;
    for(i=0;i<4;++i) if(box->epoch[i]!=epoch[i]) return 0;
    return 1;
}
/* Returns the published sequence, or zero without changing the slot. */
static u32 control_submit(volatile struct control_mailbox *box,const u32 epoch[4],
                          u32 command,u32 expected,const u8 *payload,u32 length) {
    u32 sequence, i;
    if(!control_context(box,epoch) || length>CONTROL_PAYLOAD_BYTES || (!payload && length)) return 0;
    sequence=box->request;
    if(sequence!=box->ready || sequence!=box->release || sequence>=CONTROL_SEQUENCE_MAX) return 0;
    CONTROL_BARRIER();
    for(i=0;i<4;++i) box->request_epoch[i]=epoch[i];
    box->command=command;
    box->expected_generation=expected;
    box->length=length;
    box->checksum=control_checksum(payload,length);
    box->cancel=0;
    for(i=0;i<length;++i) box->payload[i]=payload[i];
    CONTROL_BARRIER();
    box->request=sequence+1;
    return sequence+1;
}
/* Copies a complete candidate into consumer-private RAM. An accepted candidate
   still needs command, generation, content and runtime-safety validation.
   For a validation error, complete out->sequence with that error. */
static u32 control_acquire(volatile struct control_mailbox *box,const u32 epoch[4],struct control_request *out) {
    u32 sequence, checksum, i, result=CONTROL_ACCEPTED;
    CONTROL_INVALIDATE(&box->request,64);
    if(!control_context(box,epoch)) return CONTROL_EMPTY;
    sequence=box->request;
    if(!sequence || sequence>CONTROL_SEQUENCE_MAX || sequence==box->ready || sequence==box->claimed) return CONTROL_EMPTY;
    CONTROL_BARRIER();
    if(box->ready!=box->release || sequence!=box->ready+1) return CONTROL_EMPTY;
    box->claimed=sequence;
    out->sequence=sequence;
    out->command=box->command;
    out->expected_generation=box->expected_generation;
    out->length=box->length;
    checksum=box->checksum;
    for(i=0;i<4;++i) if(box->request_epoch[i]!=epoch[i]) result=CONTROL_BAD_CONTEXT;
    if(out->length>CONTROL_PAYLOAD_BYTES) result=CONTROL_BAD_LENGTH;
    if(result==CONTROL_ACCEPTED) {
        CONTROL_INVALIDATE(box->payload,CONTROL_PAYLOAD_BYTES);
        for(i=0;i<out->length;++i) out->payload[i]=box->payload[i];
        if(control_checksum(out->payload,out->length)!=checksum) result=CONTROL_BAD_DIGEST;
    }
    CONTROL_BARRIER();
    if(box->request!=sequence) result=CONTROL_CHANGED;
    return result;
}
/* Cancellation is a request to the coordinator. It cannot undo a durable
   commit; the coordinator decides whether its current phase permits it. */
static u32 control_cancelled(volatile struct control_mailbox *box,u32 sequence) {
    u32 cancelled;
    CONTROL_INVALIDATE(&box->request,64);
    cancelled=box->cancel;
    CONTROL_BARRIER();
    return cancelled==sequence && box->request==sequence && box->ready!=sequence;
}
/* Call only with the coordinator's actual outcome. Successful transport does
   not establish that data was persisted or that rendering uses the candidate. */
static u32 control_complete_response(volatile struct control_mailbox *box,u32 sequence,u32 result,u32 generation, const u8 *bytes,u32 length) {
    if(length>CONTROL_PAYLOAD_BYTES || (length && !bytes)) return 0;
    if(!sequence || box->request!=sequence || box->claimed!=sequence || box->ready==sequence) return 0;
    for(u32 i=0;i<length;++i) box->response[i]=bytes[i];
    box->response_length=length;
    box->response_checksum=control_checksum(bytes,length);
    CONTROL_CLEAN(box->response,CONTROL_PAYLOAD_BYTES);
    box->result=result;
    box->generation=generation;
    CONTROL_CLEAN(&box->claimed,64);
    CONTROL_BARRIER();
    box->ready=sequence;
    CONTROL_CLEAN(&box->claimed,64);
    return 1;
}
static u32 control_complete(volatile struct control_mailbox *box,u32 sequence,u32 result,u32 generation) {
    return control_complete_response(box,sequence,result,generation,0,0);
}
static u32 control_release(volatile struct control_mailbox *box,u32 sequence) {
    if(!sequence || box->ready!=sequence || box->request!=sequence) return 0;
    CONTROL_BARRIER();
    box->release=sequence;
    return 1;
}
/* First attach installs a nonzero runtime nonce. A restarted Linux service
   reads and reuses this nonce and any pending ticket. It must not reinitialize
   the buffer. No data mutation is available through these commands. */
static u32 control_poll_once(volatile struct control_mailbox *box,struct control_request *candidate) {
    u32 epoch[4], nonce[4], i, any=0, attached=0, result;
    for(i=0;i<4;++i) { epoch[i]=box->epoch[i]; attached|=epoch[i]; }
    /* The resident owner is the sole consumer of library requests. App must
       not claim them before the owner can perform asynchronous file I/O. */
    CONTROL_INVALIDATE(&box->request,64);
    if(box->command>=GRKIT_CONTROL_SNAPSHOT_BEGIN && box->command<=GRKIT_CONTROL_CARD_IDENTITY) return 0;
    result=control_acquire(box,epoch,candidate);
    if(result==CONTROL_EMPTY) return 0;
    if(result==CONTROL_ACCEPTED) {
        result=0;
        if(candidate->command==CONTROL_ATTACH) {
            if(candidate->length!=16) result=CONTROL_BAD_LENGTH;
            else {
                for(i=0;i<4;++i) {
                    const u8 *p=candidate->payload+4*i;
                    nonce[i]=(u32)p[0]|((u32)p[1]<<8)|((u32)p[2]<<16)|((u32)p[3]<<24);
                    any|=nonce[i];
                    if(attached && nonce[i]!=epoch[i]) result=CONTROL_BAD_CONTEXT;
                }
                if(!any) result=CONTROL_INVALID;
                if(!result) for(i=0;i<4;++i) box->epoch[i]=nonce[i];
            }
        } else if(candidate->command==CONTROL_PING) {
            if(!attached) result=CONTROL_BAD_CONTEXT;
            else if(candidate->length) result=CONTROL_BAD_LENGTH;
        } else result=CONTROL_UNSUPPORTED;
    }
    CONTROL_CLEAN(&box->magic,64);
    return control_complete(box,candidate->sequence,result,box->generation);
}
#endif
