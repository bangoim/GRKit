#ifndef GRKIT_LOOK_PUBLISH_H
#define GRKIT_LOOK_PUBLISH_H
#include "abi.h"
/* Resident owns request/input/token until App acknowledges done. App owns
   accepted/slot/row and prepared. Resident retains the validated GRF buffer
   until COMMIT or CANCEL is acknowledged. COMMIT means durable owner success. */
#define LOOK_PUBLISH_ADD 0U
#define LOOK_PUBLISH_METADATA 1U
#define LOOK_PUBLISH_REPLACE 2U
#define LOOK_PUBLISH_PREPARE 1U
#define LOOK_PUBLISH_COMMIT 2U
#define LOOK_PUBLISH_CANCEL 3U
#define LOOK_PUBLISH_OK 0U
#define LOOK_PUBLISH_BUSY 1U
#define LOOK_PUBLISH_UNSUPPORTED 2U
#define LOOK_PUBLISH_FULL 3U
#define LOOK_PUBLISH_CONFLICT 4U
#define LOOK_PUBLISH_INVALID 5U
#define LOOK_PUBLISH_RECOVERY 6U
struct look_publish {
    u8 stock[GRKIT_RECORD_COEFFICIENTS];
    u32 token[4], profile, length, generation, enabled;
    u8 name[20];
    u32 request, done, result;
    u32 accepted[4], slot, row, code, prepared, acknowledged[4];
    u32 multiaxial_regions, multiaxial_rgb, deferred_native, deferred_dial_forget;
    u32 action, manifest, manifest_length, select_off, setting_slot;
    /* App-owned retirement only blocks new selections, never capture readers. */
    u32 retired[2];
    u32 target_bank, pending_hash[8], retire_wait[2], base_count;
    u32 previous_slot, evicted_row;
};
_Static_assert(sizeof(struct look_publish)<=320,"look publication services budget");
#endif /* GRKIT_LOOK_PUBLISH_H */
#ifdef LOOK_PUBLISH_PREPARE_RECORD
#ifndef GRKIT_LOOK_PUBLISH_POLL_H
#define GRKIT_LOOK_PUBLISH_POLL_H
#ifndef LOOK_PUBLISH_BARRIER
#define LOOK_PUBLISH_BARRIER() __asm__ volatile("dmb ish" ::: "memory")
#endif
static void look_publish_poll(volatile u32 *h,volatile struct look_publish *p) {
    u32 request=p->request, result=LOOK_PUBLISH_INVALID, matches=1, acknowledged=1, any=0;
    if(!request) return;
    LOOK_PUBLISH_BARRIER();
    for(u32 i=0;i<4;++i) {
        any|=p->token[i];
        if(p->token[i]!=p->accepted[i]) matches=0;
        if(p->token[i]!=p->acknowledged[i]) acknowledged=0;
    }
    if(request==p->done && acknowledged) return;
    if(request==LOOK_PUBLISH_PREPARE && any) {
        if(p->prepared) result=LOOK_PUBLISH_BUSY;
        else {
            result=LOOK_PUBLISH_PREPARE_RECORD(h,p);
            if(!result) {
                for(u32 i=0;i<4;++i) p->accepted[i]=p->token[i];
                p->prepared=1;
                LOOK_PUBLISH_CAPTURE(h);
            }
        }
    } else if(request==LOOK_PUBLISH_COMMIT) {
        result=LOOK_PUBLISH_RECOVERY;
        if(matches && p->prepared) {
            result=LOOK_PUBLISH_COMMIT_RECORD(h,p);
            if(!result) p->prepared=0;
        }
    } else if(request==LOOK_PUBLISH_CANCEL && matches) {
        result=LOOK_PUBLISH_CONFLICT;
        if(p->prepared) {
            p->prepared=0;
            result=LOOK_PUBLISH_OK;
        }
    }
    /* A menu opened after durable commit: wait for its views to close. */
    if(request==LOOK_PUBLISH_COMMIT && result==LOOK_PUBLISH_BUSY) return;
    p->result=result;
    for(u32 i=0;i<4;++i) p->acknowledged[i]=p->token[i];
    LOOK_PUBLISH_BARRIER();
    p->done=request;
}
#endif
#endif
