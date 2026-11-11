//! 对象转换：A2L 的块结构到 `cal-core` 的 IR。
//!
//! 维度顺序是这里最容易写错的一处：A2L 的 `MATRIX_DIM` 与 `AXIS_DESCR` 都是
//! **快速维在前**（x、y、z），而 IR 的 `dims` 与 `axes` 按 DWARF 顺序（慢速维
//! 在前）。两处都要反转，且反转只在 `reverse` 这一个名字下发生。

use std::collections::BTreeMap;

use a2lfile::{
    A2lObjectName, AxisDescr, AxisDescrAttribute, AxisPts, Characteristic, CharacteristicType,
    ConversionType, Measurement, Module,
};
use openape_cal_core::{
    Address, Axis, AxisBinding, AxisKind, BoundValue, ByteOrder, ElemType, Id, Range, Role, Scale,
    Semantics, Symbol, Unit, Variable, VariableTree,
};

use crate::error::Error;
use crate::layout::{byte_order, elem_of_deposit, object_byte_order};
use openape_cal_core::Notice;

/// 转换过程中的累积状态。
struct Builder<'a> {
    module: &'a Module,
    module_byte_order: ByteOrder,
    variables: Vec<Variable>,
    owners: BTreeMap<String, &'static str>,
    axes: Vec<Axis>,
    values: Vec<BoundValue>,
    notes: Vec<Notice>,
    errors: Vec<Error>,
}

/// 把一个模块转成变量树与轴绑定。
pub(crate) fn build(
    module: &Module,
    notes: &mut Vec<Notice>,
) -> Result<(VariableTree, AxisBinding), Error> {
    let mut builder = Builder {
        module,
        module_byte_order: byte_order(module),
        variables: Vec::new(),
        owners: BTreeMap::new(),
        axes: Vec::new(),
        values: Vec::new(),
        notes: Vec::new(),
        errors: Vec::new(),
    };

    builder.collect_axes();
    builder.collect_characteristics();
    builder.collect_measurements();

    notes.append(&mut builder.notes);
    if let Some(error) = builder.errors.into_iter().next() {
        return Err(error);
    }
    let tree = VariableTree::new(builder.variables).map_err(|err| Error::DuplicateSymbol {
        symbol: err.symbol.to_string(),
        first: "较早的对象",
        second: "后出现的对象",
    })?;
    Ok((tree, AxisBinding::from_parts(builder.axes, builder.values)))
}

impl Builder<'_> {
    /// 占住一个名字。重名不是"跳过这个对象"那么简单——用户会以为读到的
    /// 是另一个对象，所以记成错误，让整个读入失败。
    fn claim(&mut self, name: &str, kind: &'static str) -> bool {
        match self.owners.get(name) {
            Some(first) => {
                self.errors.push(Error::DuplicateSymbol {
                    symbol: name.to_owned(),
                    first,
                    second: kind,
                });
                false
            }
            None => {
                self.owners.insert(name.to_owned(), kind);
                true
            }
        }
    }

    /// `AXIS_PTS`：独立存放的轴，同时也是可读的标定量。
    fn collect_axes(&mut self) {
        for declared in self.module.axis_pts.iter() {
            let name = declared.get_name().to_owned();
            match self.axis_pts_to_axis(declared) {
                Ok((axis, variable)) => {
                    if self.claim(&name, "AXIS_PTS") {
                        self.axes.push(axis);
                        self.variables.push(variable);
                    }
                }
                Err(reason) => {
                    self.notes.push(Notice::warning(
                        name.clone(),
                        format!("跳过 AXIS_PTS：{reason}"),
                    ));
                }
            }
        }
    }

    fn axis_pts_to_axis(&mut self, declared: &AxisPts) -> Result<(Axis, Variable), String> {
        let name = declared.get_name().to_owned();
        let points = u32::from(declared.max_axis_points);
        if points == 0 {
            return Err("MAX_AXIS_POINTS 为 0".to_owned());
        }
        let elem = elem_of_deposit(
            self.module,
            &declared.deposit_record,
            &name,
            &mut self.notes,
        )
        .ok_or_else(|| "元素类型不受支持".to_owned())?;
        let symbol = Symbol::new(&symbol_name(&name, declared.symbol_link.as_ref()))
            .ok_or_else(|| format!("名字 {name:?} 不能作为符号"))?;
        let id = Id::new(&name).ok_or_else(|| format!("名字 {name:?} 不能作为 id"))?;
        let address = Address::new(declared.address);
        let semantics = semantics_of(
            self.module,
            &declared.conversion,
            declared.phys_unit.as_ref().map(|unit| unit.unit.as_str()),
            declared.lower_limit,
            declared.upper_limit,
            &name,
            &mut self.notes,
        );

        let variable = Variable::new(symbol.clone(), address, elem)
            .with_dims(vec![points])
            .ok_or_else(|| "轴点数不能为 0".to_owned())?
            .with_role(Role::Characteristic)
            .with_byte_order(object_byte_order(
                declared.byte_order.as_ref(),
                self.module_byte_order,
            ))
            .with_semantics(semantics);

        let axis = Axis::new(id, symbol, address, elem, points, AxisKind::Shared)
            .ok_or_else(|| "轴点数不能为 0".to_owned())?;
        Ok((axis, variable))
    }

    /// `CHARACTERISTIC`：标定量，一维是 Curve、二维是 Map。
    fn collect_characteristics(&mut self) {
        for declared in self.module.characteristic.iter() {
            let name = declared.get_name().to_owned();
            match self.characteristic_to_value(declared) {
                Ok((Some(value), variable)) => {
                    if self.claim(&name, "CHARACTERISTIC") {
                        self.values.push(value);
                        self.variables.push(variable);
                    }
                }
                // 标量标定量（VALUE）：能读能写，但它不是 Curve 也不是 Map，
                // 没有轴可绑，所以只进变量树。
                Ok((None, variable)) => {
                    if self.claim(&name, "CHARACTERISTIC") {
                        self.variables.push(variable);
                    }
                }
                Err(reason) => {
                    self.notes.push(Notice::warning(
                        name.clone(),
                        format!("跳过 CHARACTERISTIC：{reason}"),
                    ));
                }
            }
        }
    }

    fn characteristic_to_value(
        &mut self,
        declared: &Characteristic,
    ) -> Result<(Option<BoundValue>, Variable), String> {
        let name = declared.get_name().to_owned();
        let elem = elem_of_deposit(self.module, &declared.deposit, &name, &mut self.notes)
            .ok_or_else(|| "元素类型不受支持".to_owned())?;
        let dims = self.dims_of_characteristic(declared)?;
        // 只有 CURVE 与 MAP 有"值加轴"的关系；VALUE 是标量，VAL_BLK 是没有
        // 轴的值数组，两者都只进变量树。
        let bound = matches!(
            declared.characteristic_type,
            CharacteristicType::Curve | CharacteristicType::Map
        );
        let axes = if bound {
            self.axes_of(declared, &name, elem)?
        } else {
            Vec::new()
        };
        let address = Address::new(declared.address);
        let semantics = semantics_of(
            self.module,
            &declared.conversion,
            declared.phys_unit.as_ref().map(|unit| unit.unit.as_str()),
            declared.lower_limit,
            declared.upper_limit,
            &name,
            &mut self.notes,
        );

        let symbol = symbol_of(&symbol_name(&name, declared.symbol_link.as_ref()))?;
        let variable = Variable::new(symbol.clone(), address, elem)
            .with_dims(dims.clone())
            .ok_or_else(|| "维度含 0".to_owned())?
            .with_role(Role::Characteristic)
            .with_byte_order(object_byte_order(
                declared.byte_order.as_ref(),
                self.module_byte_order,
            ))
            .with_semantics(semantics);

        if !bound {
            return Ok((None, variable));
        }
        let value = self.register_value(&name, symbol, address, elem, dims, axes)?;
        Ok((Some(value), variable))
    }

    /// 把新出现的轴并进轴表，再用它们建一条绑定值。
    fn register_value(
        &mut self,
        name: &str,
        symbol: Symbol,
        address: Address,
        elem: ElemType,
        dims: Vec<u32>,
        axes: Vec<Axis>,
    ) -> Result<BoundValue, String> {
        let axis_ids: Vec<Id> = axes.iter().map(|axis| axis.id().clone()).collect();
        for axis in axes {
            if self.axes.iter().all(|known| known.id() != axis.id()) {
                self.axes.push(axis);
            }
        }
        let id = Id::new(name).ok_or_else(|| format!("名字 {name:?} 不能作为 id"))?;
        BoundValue::new(id, symbol, address, elem, dims, axis_ids)
            .ok_or_else(|| "轴数与维度数不一致".to_owned())
    }

    fn dims_of_characteristic(&mut self, declared: &Characteristic) -> Result<Vec<u32>, String> {
        match declared.characteristic_type {
            CharacteristicType::Value => Ok(Vec::new()),
            CharacteristicType::Curve => {
                let axis = declared
                    .axis_descr
                    .first()
                    .ok_or_else(|| "CURVE 没有 AXIS_DESCR，读不出点数".to_owned())?;
                Ok(vec![u32::from(axis.max_axis_points)])
            }
            CharacteristicType::Map | CharacteristicType::ValBlk => {
                self.dims_from_matrix(declared, "矩阵")
            }
            other => Err(format!(
                "{other:?} 不在 MVP 支持范围内（只支持 VALUE、CURVE、MAP、VAL_BLK）"
            )),
        }
    }

    fn dims_from_matrix(
        &mut self,
        declared: &Characteristic,
        what: &str,
    ) -> Result<Vec<u32>, String> {
        if let Some(matrix) = declared.matrix_dim.as_ref() {
            let dims = reverse(matrix.dim_list.iter().map(|&d| u32::from(d)).collect());
            return Ok(dims);
        }
        let fallback: Vec<u32> = declared
            .axis_descr
            .iter()
            .map(|axis| u32::from(axis.max_axis_points))
            .collect();
        if fallback.is_empty() {
            return Err(format!("{what} 既没有 MATRIX_DIM 也没有 AXIS_DESCR"));
        }
        self.notes.push(Notice::warning(
            declared.get_name(),
            format!("{what} 没有 MATRIX_DIM，按 AXIS_DESCR 的点数推断维度"),
        ));
        Ok(reverse(fallback))
    }

    /// 按 `AXIS_DESCR` 解出轴。A2L 是快速维在前，IR 要反转。
    fn axes_of(
        &mut self,
        declared: &Characteristic,
        owner: &str,
        elem: ElemType,
    ) -> Result<Vec<Axis>, String> {
        let mut axes = Vec::with_capacity(declared.axis_descr.len());
        for (index, described) in declared.axis_descr.iter().enumerate() {
            axes.push(self.axis_of_descr(described, owner, elem, index)?);
        }
        Ok(reverse(axes))
    }

    fn axis_of_descr(
        &mut self,
        described: &AxisDescr,
        owner: &str,
        elem: ElemType,
        index: usize,
    ) -> Result<Axis, String> {
        let points = u32::from(described.max_axis_points);
        match described.attribute {
            AxisDescrAttribute::ComAxis | AxisDescrAttribute::ResAxis => {
                let reference = described
                    .axis_pts_ref
                    .as_ref()
                    .ok_or_else(|| format!("第 {} 个 AXIS_DESCR 缺 AXIS_PTS_REF", index + 1))?;
                let name = &reference.axis_points;
                self.axes
                    .iter()
                    .find(|axis| axis.id().as_str() == name)
                    .cloned()
                    .ok_or_else(|| format!("AXIS_PTS_REF 指向的 {name} 不存在"))
            }
            AxisDescrAttribute::StdAxis => {
                let id = Id::new(&format!("{owner}_axis{}", index + 1))
                    .ok_or_else(|| "生成的内嵌轴 id 不合法".to_owned())?;
                Axis::new(
                    id,
                    symbol_of(owner)?,
                    Address::new(0),
                    elem,
                    points,
                    AxisKind::Inline,
                )
                .ok_or_else(|| "内嵌轴点数为 0".to_owned())
            }
            other => Err(format!(
                "{other:?} 轴不在 MVP 支持范围内（只支持 COM_AXIS、RES_AXIS、STD_AXIS）"
            )),
        }
    }

    /// `MEASUREMENT`：观测量。数组按 `ARRAY_SIZE` 或 `MATRIX_DIM` 定维度。
    fn collect_measurements(&mut self) {
        for declared in self.module.measurement.iter() {
            let name = declared.get_name().to_owned();
            match self.measurement_to_variable(declared) {
                Ok(variable) => {
                    if self.claim(&name, "MEASUREMENT") {
                        self.variables.push(variable);
                    }
                }
                Err(reason) => {
                    self.notes.push(Notice::warning(
                        name.clone(),
                        format!("跳过 MEASUREMENT：{reason}"),
                    ));
                }
            }
        }
    }

    fn measurement_to_variable(&mut self, declared: &Measurement) -> Result<Variable, String> {
        let name = declared.get_name().to_owned();
        let elem = match crate::layout::elem_of_datatype(declared.datatype) {
            Some(elem) => elem,
            None => {
                return Err(format!(
                    "datatype {:?} 不在 MVP 支持范围内",
                    declared.datatype
                ));
            }
        };
        let dims = match (declared.array_size.as_ref(), declared.matrix_dim.as_ref()) {
            (Some(array), _) => vec![u32::from(array.number)],
            (None, Some(matrix)) => {
                reverse(matrix.dim_list.iter().map(|&d| u32::from(d)).collect())
            }
            (None, None) => Vec::new(),
        };
        let semantics = semantics_of(
            self.module,
            &declared.conversion,
            declared.phys_unit.as_ref().map(|unit| unit.unit.as_str()),
            declared.lower_limit,
            declared.upper_limit,
            &name,
            &mut self.notes,
        );
        let address = declared
            .ecu_address
            .as_ref()
            .map(|declared| Address::new(declared.address))
            .ok_or_else(|| "没有 ECU_ADDRESS；MVP 不做 symbol_link 到 ELF 的地址补齐".to_owned())?;
        let variable = Variable::new(
            symbol_of(&symbol_name(&name, declared.symbol_link.as_ref()))?,
            address,
            elem,
        )
        .with_role(Role::Measurement)
        .with_byte_order(object_byte_order(
            declared.byte_order.as_ref(),
            self.module_byte_order,
        ))
        .with_semantics(semantics);
        if dims.is_empty() {
            Ok(variable)
        } else {
            variable
                .with_dims(dims)
                .ok_or_else(|| "维度含 0".to_owned())
        }
    }
}

/// 符号名：`SYMBOL_LINK` 指向的 C 符号优先，没有就退回 A2L 对象名。
///
/// 两者可以不同（对象名是人起的、符号名是编译器给的），真 ECU 的 A2L 常
/// 只写前者；地址仍以对象为准，符号名只用于和 ELF 对照与显示。
fn symbol_name(object_name: &str, link: Option<&a2lfile::SymbolLink>) -> String {
    link.map(|link| link.symbol_name.clone())
        .unwrap_or_else(|| object_name.to_owned())
}

/// 快速维在前换成慢速维在前。全局唯一的一处反转。
fn reverse<T>(mut items: Vec<T>) -> Vec<T> {
    items.reverse();
    items
}

fn symbol_of(name: &str) -> Result<Symbol, String> {
    Symbol::new(name).ok_or_else(|| format!("名字 {name:?} 不能作为符号"))
}

/// 汇总单位、量程与换算。
fn semantics_of(
    module: &Module,
    conversion: &str,
    phys_unit: Option<&str>,
    lower: f64,
    upper: f64,
    subject: &str,
    notes: &mut Vec<Notice>,
) -> Semantics {
    let mut semantics = Semantics::none();
    let compu = module
        .compu_method
        .iter()
        .find(|method| method.get_name() == conversion);

    if let Some(unit) = phys_unit
        .and_then(Unit::new)
        .or_else(|| compu.and_then(|method| Unit::new(&method.unit)))
    {
        semantics = semantics.with_unit(unit);
    }
    match Range::new(lower, upper) {
        Some(range) => semantics = semantics.with_range(range),
        None => notes.push(Notice::warning(
            subject,
            format!("量程 [{lower}, {upper}] 上下限颠倒或含 NaN，按无范围处理"),
        )),
    }
    match compu.map(|method| (&method.conversion_type, method)) {
        Some((ConversionType::Linear, method)) => match method.coeffs_linear.as_ref() {
            Some(coeffs) => {
                semantics = semantics.with_scale(Scale::Linear {
                    factor: coeffs.a,
                    offset: coeffs.b,
                });
            }
            None => notes.push(Notice::warning(
                subject,
                format!("COMPU_METHOD {conversion} 声明 LINEAR 但没有系数，按原始值显示"),
            )),
        },
        Some((ConversionType::Identical, _)) | Some((ConversionType::Form, _)) | None => {}
        Some((other, _)) => notes.push(Notice::warning(
            subject,
            format!("换算类型 {other:?} 不在 MVP 支持范围内，物理值按原始值显示"),
        )),
    }
    semantics
}
