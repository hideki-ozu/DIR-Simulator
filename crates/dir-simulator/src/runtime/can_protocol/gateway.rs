//! Actual local reception snapshots immutable routes before finite application service.
use super::*;
impl Compound {
    pub(super) fn route_rules(&self, gateway: usize, version: u64) -> ModelResult<&[RouteRule]> {
        let initial = self
            .config
            .gateways
            .get(gateway)
            .ok_or("Gateway route owner absent")?;
        if initial.version == version {
            return Ok(&initial.rules);
        }
        self.config
            .route_updates
            .iter()
            .find(|update| update.gateway == initial.id && update.version == version)
            .map(|update| update.rules.as_slice())
            .ok_or_else(|| ModelError("Gateway immutable route version absent".into()))
    }
    pub(super) fn replace_routes(&mut self, c: &mut Context<'_>, index: usize) -> ModelResult {
        let update = self
            .config
            .route_updates
            .get(index)
            .ok_or("route update out of bounds")?
            .clone();
        let gateway = self
            .config
            .gateways
            .iter()
            .position(|g| g.id == update.gateway)
            .ok_or("route update Gateway absent")?;
        if update.version <= self.state.gateways[gateway].version {
            return Err("route update version did not advance".into());
        }
        self.state.gateways[gateway].version = update.version;
        self.record(c,format!("route:{}:{}",update.gateway,update.version),json!({"kind":"route_update","gateway":update.gateway,"version":update.version,"effective_ps":c.now().to_string()}))
    }
    pub(super) fn gateway_receive(
        &mut self,
        c: &mut Context<'_>,
        b: usize,
        n: usize,
        frame: ProtocolFrame,
        lineage: Lineage,
    ) -> ModelResult {
        let gateway = self.config.gateways.iter().position(|g| {
            g.ports.iter().any(|p| {
                p.bus == self.config.buses[b].id
                    && p.controller == self.config.buses[b].controllers[n].id
            })
        });
        let Some(g) = gateway else {
            return Ok(());
        };
        let version = self.state.gateways[g].version;
        let source = self.route_rules(g, version)?;
        let mut rules: Vec<_> = source
            .iter()
            .enumerate()
            .filter_map(|(index, rule)| {
                (rule.source_bus == self.config.buses[b].id
                    && frame.id() & rule.id_mask == rule.id_value
                    && rule.extended.is_none_or(|ext| ext == frame.extended())
                    && rule.remote.is_none_or(|remote| remote == frame.remote()))
                .then_some(index)
            })
            .collect();
        rules.sort_by_key(|&index| {
            (
                &source[index].id,
                &source[index].destination_bus,
                &source[index].controller,
            )
        });
        let rule_ids: Vec<_> = rules
            .iter()
            .map(|&index| source[index].id.clone())
            .collect();
        let queue = &mut self.state.gateways[g];
        queue.serial = queue
            .serial
            .checked_add(1)
            .ok_or("Gateway ingress sequence overflow")?;
        let serial = queue.serial;
        let state = if lineage.hop >= self.config.max_gateway_hops {
            "hop_limit_drop"
        } else if rules.is_empty() {
            "filtered"
        } else if queue.queue.len() >= self.config.gateways[g].ingress_capacity {
            "ingress_drop"
        } else {
            "queued"
        };
        self.record(c,format!("ingress:{}:{serial}",self.config.gateways[g].id),json!({"kind":"gateway_ingress","gateway":self.config.gateways[g].id,"serial":serial,"bus":self.config.buses[b].id,"controller":self.config.buses[b].controllers[n].id,"frame":frame,"lineage":lineage,"route_version":version,"rule_ids":rule_ids,"state":state,"received_ps":c.now().to_string()}))?;
        if state != "queued" {
            return Ok(());
        }
        let base = self.state.gateways[g]
            .queue
            .back()
            .map_or(self.state.now, |item| item.due);
        let due = base.add(Exact::integer(self.config.gateways[g].service_ps))?;
        let empty = self.state.gateways[g].queue.is_empty();
        self.state.gateways[g].queue.push_back(Ingress {
            serial,
            frame,
            lineage,
            source_bus: b,
            source_controller: n,
            version,
            rules,
            received: self.state.now,
            due,
        });
        if empty {
            self.enqueue(due, Event::Service { gateway: g, serial })?;
        }
        Ok(())
    }
    pub(super) fn gateway_service(
        &mut self,
        c: &mut Context<'_>,
        g: usize,
        serial: u64,
    ) -> ModelResult {
        let item = self
            .state
            .gateways
            .get_mut(g)
            .ok_or("Gateway service index out of bounds")?
            .queue
            .pop_front()
            .ok_or("Gateway service has no ingress owner")?;
        if item.serial != serial || item.due != self.state.now {
            return Err("Gateway service owner/deadline mismatch".into());
        }
        for &index in &item.rules {
            let rule = self
                .route_rules(g, item.version)?
                .get(index)
                .ok_or("Gateway snapshot rule absent")?
                .clone();
            self.state.branch_serial = self
                .state
                .branch_serial
                .checked_add(1)
                .ok_or("Gateway branch sequence overflow")?;
            let id = format!(
                "branch:{}:{}:{}",
                self.config.gateways[g].id, self.state.branch_serial, rule.id
            );
            let output = convert(&item.frame, &rule);
            match output {
                Err(reason)=>self.record(c,format!("branch:{id}"),json!({"kind":"gateway_branch","request_id":id,"origin_request_id":item.lineage.origin,"parent_request_id":item.lineage.parent,"route_version":item.version,"rule":rule.id,"source_frame":item.frame,"destination_protocol":rule.protocol,"state":"conversion_rejected","reason":reason,"source_length":item.frame.data().len(),"destination_limit":if rule.protocol=="cc"{8}else{64}}))?,
                Ok(frame)=>{
                    let(b,n)=self.address(&rule.destination_bus,&rule.controller)?;
                    let lineage=Lineage{origin:item.lineage.origin.clone(),parent:item.lineage.parent.clone(),hop:item.lineage.hop.checked_add(1).ok_or("Gateway hop count overflow")?,route_version:Some(item.version)};
                    let accepted=self.admission(c,b,n,Request{id:id.clone(),frame:frame.clone(),generated:c.now(),attempt:0,lineage:lineage.clone()})?;
                    self.record(c,format!("branch:{id}"),json!({"kind":"gateway_branch","request_id":id,"origin_request_id":lineage.origin,"parent_request_id":lineage.parent,"hop_count":lineage.hop,"route_version":item.version,"rule":rule.id,"source_frame":item.frame,"frame":frame,"destination_bus":rule.destination_bus,"controller":rule.controller,"state":if accepted{"queued"}else{"egress_drop"}}))?;
                }
            }
        }
        self.record(c,format!("ingress:{}:{serial}",self.config.gateways[g].id),json!({"kind":"gateway_ingress","gateway":self.config.gateways[g].id,"serial":serial,"route_version":item.version,"frame":item.frame,"lineage":item.lineage,"state":"completed","received_ps":item.received.ceil()?.to_string(),"completed_ps":c.now().to_string()}))?;
        if let Some(next) = self.state.gateways[g].queue.front() {
            let (due, serial) = (next.due, next.serial);
            self.enqueue(due, Event::Service { gateway: g, serial })?;
        }
        Ok(())
    }
}
fn convert(frame: &ProtocolFrame, rule: &RouteRule) -> Result<ProtocolFrame, String> {
    let id = rule.rewrite_id.unwrap_or(frame.id());
    let extended = rule.rewrite_extended.unwrap_or(frame.extended());
    if id > if extended { 0x1fffffff } else { 0x7ff } {
        return Err("rewritten identifier exceeds destination format width".into());
    }
    let output = if rule.protocol == "cc" {
        if frame.data().len() > 8 {
            return Err("FD payload exceeds exact Classical CAN eight-byte limit; fragmentation is unsupported".into());
        }
        ProtocolFrame::Cc {
            id,
            extended,
            remote: frame.remote(),
            dlc: match frame {
                ProtocolFrame::Cc { dlc, .. } => *dlc,
                _ => frame.data().len() as u8,
            },
            data: frame.data().to_vec(),
        }
    } else {
        if frame.remote() {
            return Err("Classical CAN RTR has no exact CAN FD remote-frame conversion".into());
        }
        ProtocolFrame::IsoFd {
            id,
            extended,
            brs: rule.brs,
            data: frame.data().to_vec(),
        }
    };
    plan(&output, false).map_err(|e| e.to_string())?;
    Ok(output)
}
