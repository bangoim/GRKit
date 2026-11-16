#ifndef GRKIT_LOOK_GUARD_H
#define GRKIT_LOOK_GUARD_H
/* The App component supplies the durable-transaction reservation check.
   Host harnesses and read-only components without an owner have no lock. */
#ifndef GRKIT_LOOK_MUTATION_ALLOWED
#define GRKIT_LOOK_MUTATION_ALLOWED(h) ((void)(h),1U)
#endif
#ifndef GRKIT_LOOK_RECORD_ALLOWED
#define GRKIT_LOOK_RECORD_ALLOWED(h,index) ((void)(h),(void)(index),1U)
#endif
#ifndef GRKIT_LOOK_DEFER_DIAL_FORGET
#define GRKIT_LOOK_DEFER_DIAL_FORGET(h,u) ((void)(h),(void)(u))
#endif
#endif
