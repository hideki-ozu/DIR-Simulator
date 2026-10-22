//! Stateful address/data/response adapters. They contain no backing bytes.
use super::*;
impl Soc {
    pub(super) fn ingress_payload(&self, t: &Transaction, i: usize) -> Vec<u8> {
        let mut p = (i as u64).to_le_bytes().to_vec();
        p.extend_from_slice(&t.request.address.to_le_bytes());
        p.extend_from_slice(&(t.request.length as u32).to_le_bytes());
        p.push(match t.request.operation {
            Operation::Read => 0,
            Operation::Write => 1,
            Operation::Atomic => 2,
        });
        p.extend_from_slice(&t.request.data);
        p
    }
    fn priority(&self, t: &Transaction, now: u64) -> (std::cmp::Reverse<u64>, u64, String, u32) {
        let age = (now - t.admitted) / self.config.cycle_ps;
        let p = (t.request.qos as u64 + age / self.config.fabric.aging_cycles).min(15);
        (
            std::cmp::Reverse(p),
            t.admitted,
            t.request.source.clone(),
            t.request.axi_id,
        )
    }
    pub(super) fn transport_ingress(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult {
        for i in 0..s.transactions.len() {
            if s.transactions[i].phase == Phase::Queued {
                let hit = self.cache_hit(s, i);
                s.transactions[i].cache_hit = hit;
                s.transactions[i].phase = if hit
                    || (s.transactions[i].request.transport != Transport::Noc
                        && s.transactions[i].request.operation != Operation::Atomic)
                {
                    Phase::OwnerWait
                } else {
                    Phase::Ingress
                };
            }
        }
        let mut order: Vec<_> = s
            .transactions
            .iter()
            .enumerate()
            .filter(|(_, t)| t.phase == Phase::Ingress)
            .map(|(i, t)| (i, self.priority(t, c.now())))
            .collect();
        order.sort_by_key(|(i, p)| (p.clone(), *i));
        if s.axi_write_owner.is_none() {
            s.axi_write_owner = order.iter().map(|(i, _)| *i).find(|i| {
                s.transactions[*i].request.transport == Transport::Axi
                    && s.transactions[*i].client.core().is_none()
                    && s.transactions[*i].request.operation != Operation::Read
            });
        }
        let mut ar = false;
        let mut aw = false;
        let mut w = false;
        let ahb_owner = s
            .transactions
            .iter()
            .enumerate()
            .find(|(_, t)| {
                t.request.transport == Transport::Ahb
                    && t.address_accepted
                    && t.phase != Phase::Done
            })
            .map(|(i, _)| i);
        let mut ahb = ahb_owner;
        for (i, _) in order {
            let t = s.transactions[i].clone();
            match t.request.transport {
                Transport::Axi => {
                    let refill = t.client.core().is_some();
                    let read = t.request.operation == Operation::Read || refill;
                    if t.request.operation == Operation::Atomic
                        && ((0..i).any(|older| {
                            s.transactions[older].phase != Phase::Done
                                && s.transactions[older].request.source == t.request.source
                                && s.transactions[older].request.axi_id == t.request.axi_id
                        }) || !s.axi.idle()
                            || s.transactions.iter().any(|x| {
                                x.request.transport == Transport::Axi
                                    && x.request.operation != Operation::Atomic
                                    && !x.cache_hit
                                    && matches!(x.phase, Phase::Coherence | Phase::Native)
                            }))
                    {
                        continue;
                    }
                    if !read && s.axi_write_owner != Some(i) {
                        continue;
                    }
                    if !t.address_accepted
                        && c.now()
                            >= self
                                .later(t.admitted, self.config.fabric.axi_address_delay_cycles)?
                        && if read { !ar } else { !aw }
                    {
                        if read {
                            ar = true;
                        } else {
                            aw = true;
                        }
                        s.transactions[i].address_accepted = true;
                        self.emit(s,c,if read{"axi_ar"}else{"axi_aw"},Some(i),json!({"address":t.request.address,"axqos":t.request.qos,"axi_id":t.request.axi_id,"awatop":t.request.atomic.as_ref().map(|a|a.awatop),"cache_refill":refill,"awlen":(t.request.data.len().max(1).div_ceil(self.config.fabric.data_bytes.min(4))-1),"awsize":self.config.fabric.data_bytes.min(4).trailing_zeros(),"awburst":t.request.atomic.as_ref().map(|a|a.burst)}))?;
                    }
                    if !read && !w && s.transactions[i].w_accepted < t.request.data.len() {
                        w = true;
                        let begin = s.transactions[i].w_accepted;
                        let end = (begin + self.config.fabric.data_bytes.min(4))
                            .min(t.request.data.len());
                        s.transactions[i].w_accepted = end;
                        self.emit(s,c,"axi_w",Some(i),json!({"offset":begin,"data":t.request.data[begin..end],"last":end==t.request.data.len(),"wstrb":vec![true;end-begin]}))?;
                    }
                    if s.transactions[i].address_accepted
                        && (read || s.transactions[i].w_accepted == t.request.data.len())
                    {
                        s.transactions[i].phase = Phase::OwnerWait;
                        if !read {
                            s.axi_write_owner = None;
                        }
                    }
                }
                Transport::Ahb => {
                    if ahb.is_none() {
                        ahb = Some(i);
                    }
                    if ahb == Some(i) && !t.address_accepted {
                        s.transactions[i].address_accepted = true;
                        s.transactions[i].phase = Phase::OwnerWait;
                        self.emit(s,c,"ahb_address",Some(i),json!({"haddr":t.request.address,"htrans":"NONSEQ","hwrite":t.request.operation!=Operation::Read,"hready":true}))?;
                    }
                }
                Transport::Noc => {
                    if !t.ingress_sent {
                        let payload = self.ingress_payload(&t, i);
                        if s.noc.submit(
                            c.now(),
                            t.request.router,
                            self.config.memory_router,
                            format!("request-{i}"),
                            payload,
                        )? {
                            s.transactions[i].ingress_sent = true;
                            self.emit(s,c,"noc_request_submitted",Some(i),json!({"router":t.request.router,"memory_router":self.config.memory_router}))?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub(super) fn transport_completions(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult {
        for (transport, id, data, error) in std::mem::take(&mut s.native_deliveries) {
            let i = id / 4097;
            let offset = id % 4097;
            let t = s
                .transactions
                .get(i)
                .ok_or("orphan native target completion")?
                .clone();
            if t.phase != Phase::Native || t.offset != offset || t.request.transport != transport {
                return Err("native completion stage/offset identity mismatch".into());
            }
            if error {
                s.transactions[i].error = Some("native_target_error".into());
                if t.request.operation == Operation::Read {
                    s.transactions[i].result = vec![0; t.request.length];
                }
                self.finish_transaction(s, c, i)?;
                continue;
            }
            if self.mailbox_accesses_completed(s, i) {
                s.transactions[i].result = if t.request.operation == Operation::Read {
                    data
                } else {
                    vec![]
                };
                s.transactions[i].offset = t.request.length;
                self.finish_transaction(s, c, i)?;
            } else if t.client.core().is_some() {
                let line = t.line.ok_or("native cache fill lost directory owner")?;
                self.install_core_data(s, c, i, line, data)?;
                s.transactions[i].offset = t.request.length;
                self.finish_transaction(s, c, i)?;
            } else {
                let address = t
                    .request
                    .address
                    .checked_add(offset as u32)
                    .ok_or("native address overflow")?;
                let count = (self.config.cache.line_bytes
                    - address as usize % self.config.cache.line_bytes)
                    .min(t.request.length - offset);
                if t.request.operation == Operation::Read {
                    if data.len() != count {
                        return Err("short native byte response".into());
                    }
                    s.transactions[i].result.extend(data);
                }
                s.transactions[i].offset += count;
                self.emit(
                    s,
                    c,
                    "native_target_completed",
                    Some(i),
                    json!({"transport":transport,"offset":offset,"completed_bytes":count}),
                )?;
                if s.transactions[i].offset == t.request.length {
                    self.finish_transaction(s, c, i)?;
                } else {
                    self.release_line(s, i)?;
                    s.transactions[i].phase = Phase::OwnerWait;
                }
            }
        }
        for (id, payload, status) in std::mem::take(&mut s.noc_deliveries) {
            let (response, text) = if let Some(i) = id.strip_prefix("response-") {
                (true, i)
            } else if let Some(i) = id.strip_prefix("request-") {
                (false, i)
            } else {
                return Err("unknown composite NoC packet".into());
            };
            let i: usize = text
                .parse()
                .map_err(|_| ModelError("invalid composite NoC identity".into()))?;
            let t = s
                .transactions
                .get(i)
                .ok_or("orphan composite NoC packet")?
                .clone();
            if status != "packet_delivered" {
                return Err("healthy composite NoC packet failed".into());
            }
            if response {
                if t.phase != Phase::Response || !t.response_sent || payload != t.wire_data {
                    return Err("NoC response identity/payload mismatch".into());
                }
                s.transactions[i].response_delivered = true;
                if t.client.core().is_some()
                    && !t.cache_hit
                    && t.error.is_none()
                    && self
                        .mailbox_index(t.request.address, t.request.length)
                        .is_none()
                {
                    self.install_core_data(
                        s,
                        c,
                        i,
                        t.line.ok_or("NoC cache response lost owner")?,
                        payload.clone(),
                    )?;
                }
                self.finish_transaction(s, c, i)?;
            } else {
                if t.phase != Phase::Ingress
                    || !t.ingress_sent
                    || payload != self.ingress_payload(&t, i)
                {
                    return Err("NoC request identity/payload mismatch".into());
                }
                s.transactions[i].ingress_delivered = true;
                s.transactions[i].phase = Phase::OwnerWait;
            }
        }
        let mut order: Vec<_> = s
            .transactions
            .iter()
            .enumerate()
            .filter(|(_, t)| t.phase == Phase::Response)
            .map(|(i, t)| (i, self.priority(t, c.now())))
            .collect();
        order.sort_by_key(|(i, p)| (p.clone(), *i));
        let mut r = false;
        let mut b = false;
        let mut ahb = false;
        for (i, _) in order {
            let t = s.transactions[i].clone();
            let ready = t.response_ready.ok_or("response without ready time")?;
            if t.cache_hit {
                if c.now() >= self.later(ready, 1)? {
                    self.finish_transaction(s, c, i)?;
                }
                continue;
            }
            match t.request.transport {
                Transport::Noc => {
                    if !t.response_sent
                        && s.noc.submit(
                            c.now(),
                            self.config.memory_router,
                            t.request.router,
                            format!("response-{i}"),
                            t.wire_data.clone(),
                        )?
                    {
                        s.transactions[i].response_sent = true;
                        self.emit(
                            s,
                            c,
                            "noc_response_submitted",
                            Some(i),
                            json!({"data":t.wire_data}),
                        )?;
                    }
                }
                Transport::Axi => {
                    let returns = t.request.operation == Operation::Read
                        || t.client.core().is_some()
                        || t.request
                            .atomic
                            .as_ref()
                            .is_some_and(|a| a.awatop & 0x20 != 0);
                    let writes =
                        t.client.core().is_none() && t.request.operation != Operation::Read;
                    if returns
                        && s.transactions[i].r_sent < t.wire_data.len().max(1)
                        && !r
                        && c.now() >= self.later(ready, t.request.r_stall_cycles)?
                    {
                        r = true;
                        let begin = t.r_sent;
                        let end = (begin + self.config.fabric.data_bytes).min(t.wire_data.len());
                        s.transactions[i].r_sent = end.max(1);
                        self.emit(s,c,"axi_r",Some(i),json!({"data":t.wire_data.get(begin..end).unwrap_or(&[]),"offset":begin,"last":end==t.wire_data.len(),"valid_since_ps":ready,"axi_id":t.request.axi_id,"status":t.error.as_deref().unwrap_or("okay")}))?;
                    }
                    if writes
                        && !t.b_sent
                        && !b
                        && c.now() >= self.later(ready, t.request.b_stall_cycles)?
                    {
                        b = true;
                        s.transactions[i].b_sent = true;
                        self.emit(s,c,"axi_b",Some(i),json!({"valid_since_ps":ready,"axi_id":t.request.axi_id,"status":t.error.as_deref().unwrap_or("okay")}))?;
                    }
                    if (!returns || s.transactions[i].r_sent >= t.wire_data.len().max(1))
                        && (!writes || s.transactions[i].b_sent)
                    {
                        self.finish_transaction(s, c, i)?;
                    }
                }
                Transport::Ahb => {
                    if !ahb
                        && c.now() >= self.later(ready, self.config.fabric.ahb_wait_cycles + 1)?
                    {
                        ahb = true;
                        let total = t.wire_data.len().max(t.request.length);
                        let begin = t.r_sent;
                        let end = (begin + self.config.fabric.data_bytes).min(total);
                        s.transactions[i].r_sent = end;
                        let data = if t.request.operation == Operation::Write
                            && t.client.core().is_none()
                        {
                            t.request.data.get(begin..end).unwrap_or(&[])
                        } else {
                            t.wire_data
                                .get(begin..end.min(t.wire_data.len()))
                                .unwrap_or(&[])
                        };
                        self.emit(s,c,"ahb_data",Some(i),json!({"haddr":t.request.address.wrapping_add(begin as u32),"hready":true,"hresp":t.error.as_deref().unwrap_or("OKAY"),"data":data,"last":end==total}))?;
                        if end == total {
                            self.finish_transaction(s, c, i)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub(super) fn submit_native(
        &self,
        s: &mut State,
        c: &mut Context<'_>,
        i: usize,
        deny: bool,
    ) -> ModelResult {
        let t = s.transactions[i].clone();
        if t.request.transport == Transport::Axi && s.transactions.iter().any(Self::atomic_reserved)
        {
            return Ok(());
        }
        let mmio = self
            .mailbox_index(t.request.address, t.request.length)
            .is_some();
        let word = mmio && !deny;
        let core = t.client.core().is_some() && !mmio;
        let address = if core && !deny {
            self.line(t.request.address)
        } else {
            t.request
                .address
                .checked_add(t.offset as u32)
                .ok_or("native target address overflow")?
        };
        let length = if word {
            4
        } else if core && !deny {
            self.config.cache.line_bytes
        } else {
            (self.config.cache.line_bytes - address as usize % self.config.cache.line_bytes)
                .min(t.request.length - t.offset)
        };
        if let Some(data) = &t.intervention {
            let line = t.line.ok_or("native dirty transfer lacks line")?;
            if !self.backend_write(
                s,
                format!("intervention:{i}:{}", t.offset),
                Some(i),
                line,
                data,
                &vec![true; data.len()],
            )? {
                return Ok(());
            }
            self.emit(
                s,
                c,
                "cache_writeback",
                Some(i),
                json!({"line":line,"data":data,"reason":"native_owner_intervention"}),
            )?;
            s.transactions[i].intervention = None;
        }
        let read = core || t.request.operation == Operation::Read;
        let data = if read {
            vec![]
        } else {
            t.request.data[t.offset..t.offset + length].to_vec()
        };
        let enables = if read || t.request.byte_enables.is_empty() {
            vec![]
        } else {
            t.request.byte_enables[t.offset..t.offset + length].to_vec()
        };
        let id = i
            .checked_mul(4097)
            .and_then(|n| n.checked_add(t.offset))
            .ok_or("native transfer identity overflow")?;
        match t.request.transport {
            Transport::Axi => s.axi.submit(
                id,
                &t.request.source,
                c.now(),
                address,
                length,
                &data,
                &enables,
                read,
                deny,
                t.request.axi_id,
                self.config.fabric.axi_address_delay_cycles,
                t.request.r_stall_cycles,
                t.request.b_stall_cycles,
                t.request.qos,
                self.is_ddr(address) || word,
                word,
            )?,
            Transport::Ahb => s.ahb.submit(
                id,
                &t.request.source,
                c.now(),
                address,
                length,
                &data,
                &enables,
                read,
                deny,
                self.is_ddr(address) || word,
                word,
            )?,
            Transport::Noc => return Err("NoC cannot use native bus service".into()),
        };
        s.transactions[i].phase = Phase::Native;
        self.emit(s,c,"native_target_submitted",Some(i),json!({"transport":t.request.transport,"address":address,"length":length,"read":read,"deny":deny}))?;
        Ok(())
    }
}
