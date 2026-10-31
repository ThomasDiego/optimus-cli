//! Socket filter completing Landlock: only IP and netlink sockets, so no unix
//! socket (docker.sock, ssh-agent, D-Bus) and no io_uring, which would open
//! sockets out of sight. Without network approval, TCP only: Landlock blocks
//! its connections, and UDP (DNS) is refused here. On kernels whose Landlock
//! cannot block TCP, no IP socket at all.

use std::io;

use libc::{BPF_ABS, BPF_ALU, BPF_AND, BPF_JEQ, BPF_JGE, BPF_JMP, BPF_K, BPF_LD, BPF_RET, BPF_W, sock_filter, sock_fprog};

#[cfg(target_arch = "x86_64")]
const ARCH: u32 = 0xC000_003E;
#[cfg(target_arch = "aarch64")]
const ARCH: u32 = 0xC000_00B7;
/// Other architectures get no filter, see `filter`.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const ARCH: u32 = 0;
const ALLOW: u32 = libc::SECCOMP_RET_ALLOW;
const DENY: u32 = libc::SECCOMP_RET_ERRNO | libc::EPERM as u32;
/// Foreign-architecture syscalls, which the checks below cannot read.
const KILL: u32 = libc::SECCOMP_RET_KILL_PROCESS;

/// Offsets in `struct seccomp_data`.
const NR: u32 = 0;
const ARCH_AT: u32 = 4;
const fn arg(i: u32) -> u32 {
    16 + 8 * i
}

#[derive(Clone, Copy)]
enum To {
    Skip(u8),
    Ret(u32),
}

enum Op {
    Load(u32),
    And(u32),
    Jeq(u32, To, To),
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
    Jge(u32, To, To),
    Ret(u32),
}

/// `None` on architectures without a filter.
pub fn filter(net: bool, tcp_guarded: bool) -> Option<Vec<sock_filter>> {
    if !cfg!(any(target_arch = "x86_64", target_arch = "aarch64")) {
        return None;
    }
    use To::{Ret, Skip};
    let next = Skip(0);
    let mut ops = vec![Op::Load(ARCH_AT), Op::Jeq(ARCH, next, Ret(KILL)), Op::Load(NR)];
    #[cfg(target_arch = "x86_64")]
    ops.push(Op::Jge(0x4000_0000, Ret(KILL), next)); // x32 syscalls
    ops.extend([
        Op::Jeq(libc::SYS_io_uring_setup as u32, Ret(DENY), next),
        Op::Jeq(libc::SYS_socket as u32, next, Ret(ALLOW)),
        Op::Load(arg(0)),
        Op::Jeq(libc::AF_NETLINK as u32, Ret(ALLOW), next),
        Op::Jeq(libc::AF_INET as u32, Skip(1), next),
        Op::Jeq(libc::AF_INET6 as u32, next, Ret(DENY)),
    ]);
    if net {
        ops.push(Op::Ret(ALLOW));
    } else if !tcp_guarded {
        ops.push(Op::Ret(DENY));
    } else {
        ops.extend([
            Op::Load(arg(2)),
            Op::Jeq(0, Skip(1), next),
            Op::Jeq(libc::IPPROTO_TCP as u32, next, Ret(DENY)),
            Op::Load(arg(1)),
            Op::And(0xf), // SOCK_TYPE_MASK: drops SOCK_NONBLOCK and SOCK_CLOEXEC
            Op::Jeq(libc::SOCK_STREAM as u32, Ret(ALLOW), Ret(DENY)),
        ]);
    }
    Some(assemble(&ops))
}

/// Called between fork and exec: no allocation.
pub fn install(filter: &[sock_filter]) -> io::Result<()> {
    let prog = sock_fprog { len: filter.len() as u16, filter: filter.as_ptr().cast_mut() };
    // SAFETY: prog points to a live filter, which the kernel copies.
    if unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER as libc::c_ulong, &raw const prog) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Resolves `Ret` jump targets to three shared return instructions at the end.
fn assemble(ops: &[Op]) -> Vec<sock_filter> {
    let tail = [ALLOW, DENY, KILL];
    let jump = |i: usize, to: To| match to {
        To::Skip(n) => n,
        To::Ret(r) => (ops.len() + tail.iter().position(|&t| t == r).expect("known return") - i - 1) as u8,
    };
    let stmt = |code: u32, k: u32| sock_filter { code: code as u16, jt: 0, jf: 0, k };
    let jmp = |code: u32, k: u32, jt: u8, jf: u8| sock_filter { code: (BPF_JMP | code | BPF_K) as u16, jt, jf, k };
    let body = ops.iter().enumerate().map(|(i, op)| match *op {
        Op::Load(at) => stmt(BPF_LD | BPF_W | BPF_ABS, at),
        Op::And(k) => stmt(BPF_ALU | BPF_AND | BPF_K, k),
        Op::Jeq(k, t, f) => jmp(BPF_JEQ, k, jump(i, t), jump(i, f)),
        Op::Jge(k, t, f) => jmp(BPF_JGE, k, jump(i, t), jump(i, f)),
        Op::Ret(r) => stmt(BPF_RET | BPF_K, r),
    });
    body.chain(tail.map(|r| stmt(BPF_RET | BPF_K, r))).collect()
}
