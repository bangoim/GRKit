#include <assert.h>
#include <stdint.h>
#include <string.h>
typedef uint32_t u32;
typedef uint8_t u8;
#define CONTROL_BARRIER() __atomic_thread_fence(__ATOMIC_SEQ_CST)
static void cache_check(volatile void *address,u32 length,int clean);
#define CONTROL_INVALIDATE(address,length) cache_check(address,length,0)
#define CONTROL_CLEAN(address,length) cache_check(address,length,1)
#include "control_mailbox.h"

static struct control_mailbox box __attribute__((aligned(64)));
static struct control_request candidate;
static const u32 epoch[4]={1,2,3,4};

static void cache_check(volatile void *address,u32 length,int clean) {
    uintptr_t offset=(uintptr_t)address-(uintptr_t)&box;
    assert(((uintptr_t)address&63)==0 && (length&63)==0);
    if(clean) assert(((offset==0 || offset==128) && length==64) || (offset==8896 && length==8704));
    else assert((offset==64 && length==64) || (offset==192 && length==8704));
}

int main(void) {
    u8 content[CONTROL_PAYLOAD_BYTES];
    const u32 other[4]={1,2,3,5};
    u32 sequence;
    memset(content,0x71,sizeof(content));
    assert(sizeof(box)==17600);
    control_init(&box,epoch,8);
    assert(control_acquire(&box,epoch,&candidate)==CONTROL_EMPTY);
    assert(!control_submit(&box,other,1,8,content,4));
    assert(!control_submit(&box,epoch,1,8,content,CONTROL_PAYLOAD_BYTES+1));
    assert(!control_submit(&box,epoch,1,8,0,1));
    assert(box.request==0);

    sequence=control_submit(&box,epoch,1,8,content,sizeof(content));
    assert(sequence==1);
    assert(!control_submit(&box,epoch,1,8,content,1));
    assert(!control_release(&box,sequence));
    assert(control_acquire(&box,epoch,&candidate)==CONTROL_ACCEPTED);
    assert(candidate.sequence==1 && candidate.expected_generation==8);
    assert(candidate.length==sizeof(content) && !memcmp(candidate.payload,content,sizeof(content)));
    /* A poll must not execute the same request twice. */
    assert(control_acquire(&box,epoch,&candidate)==CONTROL_EMPTY);
    assert(!control_cancelled(&box,sequence));
    box.cancel=sequence;
    assert(control_cancelled(&box,sequence));
    assert(!control_complete(&box,sequence+1,0,9));
    assert(!control_complete_response(&box,sequence,0,9,content,CONTROL_PAYLOAD_BYTES+1));
    assert(control_complete_response(&box,sequence,0,9,content,7));
    assert(box.response_length==7 && box.response_checksum==control_checksum(content,7));
    assert(!memcmp((const void *)box.response,content,7));
    assert(box.generation==9 && box.result==0 && box.ready==1);
    assert(!control_cancelled(&box,sequence));
    assert(!control_complete(&box,sequence,1,10));
    assert(!control_submit(&box,epoch,1,9,content,1));
    assert(control_release(&box,sequence));

    /* App must leave resident library work unclaimed. */
    assert(control_submit(&box,epoch,GRKIT_CONTROL_SNAPSHOT_BEGIN,9,0,0)==2);
    assert(!control_poll_once(&box,&candidate));
    assert(box.claimed==1 && box.ready==1);
    assert(control_acquire(&box,epoch,&candidate)==CONTROL_ACCEPTED);
    assert(control_complete(&box,2,CONTROL_UNSUPPORTED,9));
    assert(control_release(&box,2));
    /* Reset only inside this isolated contract fixture. */
    box.request=box.ready=box.release=box.claimed=1;

    /* Torn/corrupt content is rejected before the coordinator sees it. */
    assert(control_submit(&box,epoch,1,9,content,2)==2);
    box.payload[0]^=1;
    assert(control_acquire(&box,epoch,&candidate)==CONTROL_BAD_DIGEST);
    assert(control_complete(&box,2,CONTROL_BAD_DIGEST,9));
    assert(control_release(&box,2));
    assert(control_submit(&box,epoch,1,9,content,2)==3);
    box.length=CONTROL_PAYLOAD_BYTES+1;
    assert(control_acquire(&box,epoch,&candidate)==CONTROL_BAD_LENGTH);
    assert(control_complete(&box,3,CONTROL_BAD_LENGTH,9));
    assert(control_release(&box,3));
    assert(control_submit(&box,epoch,1,9,content,2)==4);
    box.request_epoch[0]=99;
    assert(control_acquire(&box,epoch,&candidate)==CONTROL_BAD_CONTEXT);
    assert(control_complete(&box,4,CONTROL_BAD_CONTEXT,9));
    assert(control_release(&box,4));
    assert(control_submit(&box,epoch,1,9,0,0)==5);
    assert(control_acquire(&box,epoch,&candidate)==CONTROL_ACCEPTED);
    assert(candidate.length==0);
    assert(control_complete(&box,5,0,9));
    assert(control_release(&box,5));

    /* Sequence exhaustion requires a new runtime context, never a wrap. */
    box.request=box.ready=box.release=CONTROL_SEQUENCE_MAX;
    assert(!control_submit(&box,epoch,1,9,0,0));
    assert(control_checksum((const u8 *)"123456789",9)==0xcbf43926U);
    /* The timer accepts an initial nonce once and rejects a later takeover.
       A service restart can resume the same ticket/epoch without clearing it. */
    {
        const u32 unattached[4]={0,0,0,0};
        const u8 nonce[16]={1,0,0,0,2,0,0,0,3,0,0,0,4,0,0,0};
        u8 changed_nonce[16];
        memcpy(changed_nonce,nonce,16); changed_nonce[0]=9;
        control_init(&box,unattached,1);
        assert(control_submit(&box,unattached,CONTROL_PING,0,0,0)==1);
        assert(control_poll_once(&box,&candidate));
        assert(box.result==CONTROL_BAD_CONTEXT);
        assert(control_release(&box,1));
        assert(control_submit(&box,unattached,CONTROL_ATTACH,0,nonce,16)==2);
        assert(control_poll_once(&box,&candidate));
        assert(box.result==0 && control_context(&box,epoch));
        assert(control_release(&box,2));
        assert(control_submit(&box,epoch,CONTROL_PING,0,0,0)==3);
        /* A restarted producer sees an existing request, not an empty slot. */
        assert(!control_submit(&box,epoch,CONTROL_PING,0,0,0));
        assert(control_poll_once(&box,&candidate));
        assert(box.result==0 && box.generation==1);
        assert(control_release(&box,3));
        assert(control_submit(&box,epoch,CONTROL_ATTACH,0,changed_nonce,16)==4);
        assert(control_poll_once(&box,&candidate));
        assert(box.result==CONTROL_BAD_CONTEXT && control_context(&box,epoch));
        assert(control_release(&box,4));
        assert(control_submit(&box,epoch,999,0,0,0)==5);
        assert(control_poll_once(&box,&candidate));
        assert(box.result==CONTROL_UNSUPPORTED && box.generation==1);
        assert(control_release(&box,5));
    }
    return 0;
}
