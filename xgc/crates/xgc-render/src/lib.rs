//! XGC 渲染图骨架：pass 声明读 / 写资源，按数据依赖做确定性拓扑排序。
//!
//! 这一层不碰 wgpu：先把 pass 顺序、临时候选和环检测做成可测试的纯逻辑。
//! 资源名是稳定的字符串 key；同一资源先写后读、读后写、写后写都会形成边。

#![deny(unsafe_code)]

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fmt;

mod assets;
mod draw;
mod glb;
mod gpu;
mod mesh;
mod pipeline;
mod resources;

pub use assets::MeshCache;
pub use draw::{build_draw_list, DrawItem, DrawList};
pub use glb::{load_glb, MeshError};
pub use gpu::{GpuContext, GpuError, GpuResource, GpuResourceTable};
pub use mesh::{builtin_mesh, Mesh};
pub use pipeline::{RenderCache, TrianglePipeline};
pub use resources::{ResourceDesc, ResourceKind, ResourceTable, ResourceUsage};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pass {
    pub name: String,
    pub reads: Vec<String>,
    pub writes: Vec<String>,
}

impl Pass {
    pub fn new(name: impl Into<String>, reads: &[&str], writes: &[&str]) -> Self {
        Self {
            name: name.into(),
            reads: reads
                .iter()
                .map(|resource| (*resource).to_string())
                .collect(),
            writes: writes
                .iter()
                .map(|resource| (*resource).to_string())
                .collect(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum GraphError {
    DuplicatePass(String),
    DuplicateResource(String),
    UnknownResource {
        pass: String,
        resource: String,
    },
    UnreadableResource {
        pass: String,
        resource: String,
    },
    UnwritableResource {
        pass: String,
        resource: String,
    },
    Execute {
        pass: String,
        message: String,
    },
    MultipleWriters {
        resource: String,
        first: String,
        second: String,
    },
    Cycle(Vec<String>),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicatePass(name) => write!(f, "渲染 pass 重名：{name}"),
            Self::DuplicateResource(name) => write!(f, "渲染资源重名：{name}"),
            Self::UnknownResource { pass, resource } => {
                write!(f, "pass {pass} 引用了未声明资源 {resource}")
            }
            Self::UnreadableResource { pass, resource } => {
                write!(f, "pass {pass} 读取 {resource}，但资源没有可读用法")
            }
            Self::UnwritableResource { pass, resource } => {
                write!(f, "pass {pass} 写入 {resource}，但资源没有可写用法")
            }
            Self::Execute { pass, message } => {
                write!(f, "执行 pass {pass} 失败：{message}")
            }
            Self::MultipleWriters {
                resource,
                first,
                second,
            } => write!(
                f,
                "资源 {resource} 有多个写者：{first} / {second}（当前骨架要求单写者）"
            ),
            Self::Cycle(names) => write!(f, "渲染图有环：{}", names.join(" -> ")),
        }
    }
}

impl std::error::Error for GraphError {}

#[derive(Clone, Debug, Default)]
pub struct RenderGraph {
    passes: Vec<Pass>,
    resources: ResourceTable,
}

impl RenderGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, pass: Pass) -> Result<(), GraphError> {
        if self
            .passes
            .iter()
            .any(|existing| existing.name == pass.name)
        {
            return Err(GraphError::DuplicatePass(pass.name));
        }
        self.passes.push(pass);
        Ok(())
    }

    pub fn add_resource(&mut self, resource: ResourceDesc) -> Result<(), GraphError> {
        self.resources
            .add(resource)
            .map_err(GraphError::DuplicateResource)
    }

    pub fn resources(&self) -> &ResourceTable {
        &self.resources
    }

    pub fn validate_resources(&self) -> Result<(), GraphError> {
        for pass in &self.passes {
            for resource in &pass.reads {
                let Some(desc) = self.resources.get(resource) else {
                    return Err(GraphError::UnknownResource {
                        pass: pass.name.clone(),
                        resource: resource.clone(),
                    });
                };
                if !desc.readable() {
                    return Err(GraphError::UnreadableResource {
                        pass: pass.name.clone(),
                        resource: resource.clone(),
                    });
                }
            }
            for resource in &pass.writes {
                let Some(desc) = self.resources.get(resource) else {
                    return Err(GraphError::UnknownResource {
                        pass: pass.name.clone(),
                        resource: resource.clone(),
                    });
                };
                if !desc.writable() {
                    return Err(GraphError::UnwritableResource {
                        pass: pass.name.clone(),
                        resource: resource.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    pub fn passes(&self) -> &[Pass] {
        &self.passes
    }

    pub fn len(&self) -> usize {
        self.passes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.passes.is_empty()
    }

    /// 返回 pass 下标顺序。同一层可选时取注册顺序最靠前的，保证确定性。
    pub fn order(&self) -> Result<Vec<usize>, GraphError> {
        self.validate_resources()?;

        let mut writers: HashMap<&str, usize> = HashMap::new();
        for (index, pass) in self.passes.iter().enumerate() {
            let mut seen = HashSet::new();
            for resource in &pass.writes {
                if !seen.insert(resource.as_str()) {
                    continue;
                }
                if let Some(first) = writers.insert(resource.as_str(), index) {
                    return Err(GraphError::MultipleWriters {
                        resource: resource.clone(),
                        first: self.passes[first].name.clone(),
                        second: pass.name.clone(),
                    });
                }
            }
        }

        let mut edges = HashSet::new();
        for (index, pass) in self.passes.iter().enumerate() {
            let mut seen = HashSet::new();
            for resource in &pass.reads {
                if !seen.insert(resource.as_str()) {
                    continue;
                }
                if let Some(writer) = writers.get(resource.as_str()) {
                    if *writer != index {
                        edges.insert((*writer, index));
                    }
                }
            }
        }

        let mut indegree = vec![0usize; self.passes.len()];
        let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); self.passes.len()];
        for (from, to) in edges {
            outgoing[from].push(to);
            indegree[to] += 1;
        }
        for targets in &mut outgoing {
            targets.sort_unstable();
        }

        let mut ready = BinaryHeap::new();
        for (index, degree) in indegree.iter().enumerate() {
            if *degree == 0 {
                ready.push(Reverse(index));
            }
        }

        let mut order = Vec::with_capacity(self.passes.len());
        while let Some(Reverse(index)) = ready.pop() {
            order.push(index);
            for target in &outgoing[index] {
                indegree[*target] -= 1;
                if indegree[*target] == 0 {
                    ready.push(Reverse(*target));
                }
            }
        }

        if order.len() != self.passes.len() {
            let mut cycle: Vec<String> = indegree
                .iter()
                .enumerate()
                .filter(|(_, degree)| **degree > 0)
                .map(|(index, _)| self.passes[index].name.clone())
                .collect();
            cycle.sort();
            return Err(GraphError::Cycle(cycle));
        }
        Ok(order)
    }

    pub fn order_names(&self) -> Result<Vec<String>, GraphError> {
        Ok(self
            .order()?
            .into_iter()
            .map(|index| self.passes[index].name.clone())
            .collect())
    }

    /// 先排序再逐 pass 执行；执行失败时带上 pass 名，调用方不用自己拼上下文。
    pub fn execute<E: PassExecutor>(&self, executor: &mut E) -> Result<Vec<String>, GraphError> {
        let order = self.order()?;
        let mut executed = Vec::with_capacity(order.len());
        for index in order {
            let pass = &self.passes[index];
            executor
                .execute(pass)
                .map_err(|message| GraphError::Execute {
                    pass: pass.name.clone(),
                    message,
                })?;
            executed.push(pass.name.clone());
        }
        Ok(executed)
    }
}

pub trait PassExecutor {
    fn execute(&mut self, pass: &Pass) -> Result<(), String>;
}

impl<F> PassExecutor for F
where
    F: FnMut(&Pass) -> Result<(), String>,
{
    fn execute(&mut self, pass: &Pass) -> Result<(), String> {
        self(pass)
    }
}
