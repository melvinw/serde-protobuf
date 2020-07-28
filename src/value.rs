//! Types for representing runtime Protobuf values.
use std::collections;

use protobuf;
use protobuf::stream::wire_format;

use crate::descriptor;
use crate::error;

/// Any protobuf value.
#[derive(Clone, Debug)]
pub enum Value {
    /// A boolean value.
    Bool(bool),
    /// A 32-bit signed integer.
    I32(i32),
    /// A 64-bit signed integer.
    I64(i64),
    /// A 32-bit unsigned integer.
    U32(u32),
    /// A 64-bit unsigned integer.
    U64(u64),
    /// A 32-bit floating point value.
    F32(f32),
    /// A 64-bit floating point value.
    F64(f64),
    /// A byte vector.
    Bytes(Vec<u8>),
    /// A string.
    String(String),
    /// An enum value.
    Enum(i32),
    /// A message.
    Message(Message),
}

/// A message value.
#[derive(Clone, Debug)]
pub struct Message {
    /// Known fields on the message.
    pub fields: collections::BTreeMap<i32, Field>,
    /// Unknown fields on the message.
    pub unknown: protobuf::UnknownFields,

    size: protobuf::CachedSize,
}

/// A message field value.
#[derive(Clone, Debug)]
pub enum Field {
    /// A field with a single value.
    Singular(Option<Value>),
    /// A field with several (repeated) values.
    Repeated(Vec<Value>),
}

impl Message {
    /// Creates a message given a Protobuf descriptor.
    #[inline]
    pub fn new(message: &descriptor::MessageDescriptor) -> Message {
        let mut m = Message {
            fields: collections::BTreeMap::new(),
            unknown: protobuf::UnknownFields::new(),
            size: Default::default(),
        };

        for field in message.fields() {
            m.fields.insert(
                field.number(),
                if field.is_repeated() {
                    Field::Repeated(Vec::new())
                } else {
                    Field::Singular(field.default_value().cloned())
                },
            );
        }

        m
    }

    /// Merge data from the given input stream into this message.
    #[inline]
    pub fn merge_from(
        &mut self,
        descriptors: &descriptor::Descriptors,
        message: &descriptor::MessageDescriptor,
        input: &mut protobuf::CodedInputStream,
    ) -> error::Result<()> {
        while !input.eof()? {
            let (number, wire_type) = input.read_tag_unpack()?;

            if let Some(field) = message.field_by_number(number as i32) {
                let value = self.ensure_field(field);
                value.merge_from(descriptors, field, input, wire_type)?;
            } else {
                use protobuf::rt::read_unknown_or_skip_group as u;
                u(number, wire_type, input, &mut self.unknown)?;
            }
        }
        Ok(())
    }

    // Sum of all field sizes
    fn compute_size(&self) -> u32 {
        let mut size = 0;
        for (tag, field) in &self.fields {
            size += field.size_with_tag(*tag as u32);
        }
        size += protobuf::rt::unknown_fields_size(&self.unknown);
        self.size.set(size);
        size
    }

    /// Write this message to the given output stream.
    pub fn write_to(&self, os: &mut protobuf::CodedOutputStream) -> error::Result<()> {
        self.compute_size();
        for (tag, field) in &self.fields {
            field.write_to_with_tag(*tag as u32, os, false)?;
        }
        os.write_unknown_fields(&self.unknown)?;
        Ok(())
    }

    /// Write this message to a byte vector.
    pub fn write_to_bytes(&self) -> error::Result<Vec<u8>> {
        let mut v = Vec::new();
        let mut stream = protobuf::CodedOutputStream::vec(&mut v);
        self.write_to(&mut stream)?;
        stream.flush()?;
        Ok(v)
    }

    #[inline]
    fn ensure_field(&mut self, field: &descriptor::FieldDescriptor) -> &mut Field {
        self.fields
            .entry(field.number())
            .or_insert_with(|| Field::new(field))
    }
}

impl Field {
    /// Creates a field given a Protobuf descriptor.
    #[inline]
    pub fn new(field: &descriptor::FieldDescriptor) -> Field {
        if field.is_repeated() {
            Field::Repeated(Vec::new())
        } else {
            Field::Singular(None)
        }
    }

    /// Merge data from the given input stream into this field.
    #[inline]
    pub fn merge_from(
        &mut self,
        descriptors: &descriptor::Descriptors,
        field: &descriptor::FieldDescriptor,
        input: &mut protobuf::CodedInputStream,
        wire_type: protobuf::stream::wire_format::WireType,
    ) -> error::Result<()> {
        // Make the type dispatch below more compact
        use crate::descriptor::FieldType::*;
        use protobuf::stream::wire_format::WireType::*;
        use protobuf::CodedInputStream as I;

        // Singular scalar
        macro_rules! ss {
            ($expected_wire_type:expr, $visit_func:expr, $reader:expr) => {
                self.merge_scalar(input, wire_type, $expected_wire_type, $visit_func, $reader)
            };
        }

        // Packable scalar
        macro_rules! ps {
            ($expected_wire_type:expr, $visit_func:expr, $reader:expr) => {
                self.merge_packable_scalar(
                    input,
                    wire_type,
                    $expected_wire_type,
                    $visit_func,
                    $reader,
                )
            };
            ($expected_wire_type:expr, $size:expr, $visit_func:expr, $reader:expr) => {
                // TODO: use size to pre-allocate buffer space
                self.merge_packable_scalar(
                    input,
                    wire_type,
                    $expected_wire_type,
                    $visit_func,
                    $reader,
                )
            };
        }

        match field.field_type(descriptors) {
            Bool => ps!(WireTypeVarint, Value::Bool, I::read_bool),
            Int32 => ps!(WireTypeVarint, Value::I32, I::read_int32),
            Int64 => ps!(WireTypeVarint, Value::I64, I::read_int64),
            SInt32 => ps!(WireTypeVarint, Value::I32, I::read_sint32),
            SInt64 => ps!(WireTypeVarint, Value::I64, I::read_sint64),
            UInt32 => ps!(WireTypeVarint, Value::U32, I::read_uint32),
            UInt64 => ps!(WireTypeVarint, Value::U64, I::read_uint64),
            Fixed32 => ps!(WireTypeFixed32, 4, Value::U32, I::read_fixed32),
            Fixed64 => ps!(WireTypeFixed64, 8, Value::U64, I::read_fixed64),
            SFixed32 => ps!(WireTypeFixed32, 4, Value::I32, I::read_sfixed32),
            SFixed64 => ps!(WireTypeFixed64, 8, Value::I64, I::read_sfixed64),
            Float => ps!(WireTypeFixed32, 4, Value::F32, I::read_float),
            Double => ps!(WireTypeFixed64, 8, Value::F64, I::read_double),
            Bytes => ss!(WireTypeLengthDelimited, Value::Bytes, I::read_bytes),
            String => ss!(WireTypeLengthDelimited, Value::String, I::read_string),
            Enum(_) => self.merge_enum(input, wire_type),
            Message(ref m) => self.merge_message(input, descriptors, m, wire_type),
            Group => unimplemented!(),
            UnresolvedEnum(e) => Err(error::Error::UnknownEnum { name: e.to_owned() }),
            UnresolvedMessage(m) => Err(error::Error::UnknownMessage { name: m.to_owned() }),
        }
    }

    #[inline]
    fn size_with_tag(&self, tag: u32) -> u32 {
        match self {
            Self::Singular(Some(Value::Bool(b))) => match b {
                true => 2,
                _ => 0,
            },
            Self::Singular(Some(Value::I32(x))) => match *x {
                0 => 0,
                _ => protobuf::rt::value_size(tag, *x, wire_format::WireTypeVarint),
            },
            Self::Singular(Some(Value::I64(x))) => match *x {
                0 => 0,
                _ => protobuf::rt::value_size(tag, *x, wire_format::WireTypeVarint),
            },
            Self::Singular(Some(Value::U32(x))) => match *x {
                0 => 0,
                _ => protobuf::rt::value_size(tag, *x, wire_format::WireTypeVarint),
            },
            Self::Singular(Some(Value::U64(x))) => match *x {
                0 => 0,
                _ => protobuf::rt::value_size(tag, *x, wire_format::WireTypeVarint),
            },
            Self::Singular(Some(Value::F32(_))) => 5,
            Self::Singular(Some(Value::F64(_))) => 9,
            Self::Singular(Some(Value::Bytes(v))) => protobuf::rt::bytes_size(tag, &v),
            Self::Singular(Some(Value::String(s))) => protobuf::rt::string_size(tag, &s),
            Self::Singular(Some(Value::Enum(x))) => match *x {
                0 => 0,
                _ => protobuf::rt::value_size(tag, *x, wire_format::WireTypeVarint),
            },
            Self::Singular(Some(Value::Message(m))) => m.compute_size(),
            Self::Repeated(v) => {
                let mut size = 0;
                for x in v {
                    // TODO: Avoid cloning here.
                    size += Field::Singular(Some(x.clone())).size_with_tag(tag);
                }
                size
            }
            Self::Singular(None) => 0,
        }
    }

    /// Write this field to the given output stream.
    #[inline]
    pub fn write_to_with_tag(
        &self,
        tag: u32,
        os: &mut protobuf::CodedOutputStream,
        repeated_elem: bool,
    ) -> error::Result<()> {
        match self {
            Self::Singular(Some(Value::Bool(b))) => {
                if *b || repeated_elem {
                    os.write_bool(tag, true)?;
                }
                Ok(())
            }
            Self::Singular(Some(Value::I32(x))) => {
                if *x != 0 || repeated_elem {
                    os.write_int32(tag, *x)?;
                }
                Ok(())
            }
            Self::Singular(Some(Value::I64(x))) => {
                if *x != 0 || repeated_elem {
                    os.write_int64(tag, *x)?;
                }
                Ok(())
            }
            Self::Singular(Some(Value::U32(x))) => {
                if *x != 0 || repeated_elem {
                    os.write_uint32(tag, *x)?;
                }
                Ok(())
            }
            Self::Singular(Some(Value::U64(x))) => {
                if *x != 0 || repeated_elem {
                    os.write_uint64(tag, *x)?;
                }
                Ok(())
            }
            Self::Singular(Some(Value::F32(x))) => {
                if *x != 0 as f32 || repeated_elem {
                    os.write_float(tag, *x)?;
                }
                Ok(())
            }
            Self::Singular(Some(Value::F64(x))) => {
                if *x != 0 as f64 || repeated_elem {
                    os.write_double(tag, *x)?;
                }
                Ok(())
            }
            Self::Singular(Some(Value::Bytes(v))) => {
                if !v.is_empty() {
                    os.write_bytes(tag, v.as_slice())?;
                }
                Ok(())
            }
            Self::Singular(Some(Value::String(s))) => {
                if !s.is_empty() || repeated_elem {
                    os.write_string(tag, &s)?;
                }
                Ok(())
            }
            Self::Singular(Some(Value::Enum(x))) => {
                if *x != 0 || repeated_elem {
                    os.write_enum(tag, *x)?;
                }
                Ok(())
            }
            Self::Singular(Some(Value::Message(m))) => {
                os.write_tag(tag, protobuf::wire_format::WireTypeLengthDelimited)?;
                os.write_raw_varint32(m.size.get())?;
                m.write_to(os)?;
                Ok(())
            }
            Self::Repeated(v) => {
                for x in v {
                    // TODO: Avoid cloning here.
                    Field::Singular(Some(x.clone())).write_to_with_tag(tag, os, true)?;
                }
                Ok(())
            }
            Self::Singular(None) => Ok(()),
        }
    }

    #[inline]
    fn merge_scalar<'a, A, V, R>(
        &mut self,
        input: &mut protobuf::CodedInputStream<'a>,
        actual_wire_type: wire_format::WireType,
        expected_wire_type: wire_format::WireType,
        value_ctor: V,
        reader: R,
    ) -> error::Result<()>
    where
        V: Fn(A) -> Value,
        R: Fn(&mut protobuf::CodedInputStream<'a>) -> protobuf::ProtobufResult<A>,
    {
        if expected_wire_type == actual_wire_type {
            self.put(value_ctor(reader(input)?));
            Ok(())
        } else {
            Err(error::Error::BadWireType {
                wire_type: actual_wire_type,
            })
        }
    }

    #[inline]
    fn merge_packable_scalar<'a, A, V, R>(
        &mut self,
        input: &mut protobuf::CodedInputStream<'a>,
        actual_wire_type: wire_format::WireType,
        expected_wire_type: wire_format::WireType,
        value_ctor: V,
        reader: R,
    ) -> error::Result<()>
    where
        V: Fn(A) -> Value,
        R: Fn(&mut protobuf::CodedInputStream<'a>) -> protobuf::ProtobufResult<A>,
    {
        if wire_format::WireType::WireTypeLengthDelimited == actual_wire_type {
            let len = input.read_raw_varint64()?;

            let old_limit = input.push_limit(len)?;
            while !input.eof()? {
                self.put(value_ctor(reader(input)?));
            }
            input.pop_limit(old_limit);

            Ok(())
        } else {
            self.merge_scalar(
                input,
                actual_wire_type,
                expected_wire_type,
                value_ctor,
                reader,
            )
        }
    }

    #[inline]
    fn merge_enum(
        &mut self,
        input: &mut protobuf::CodedInputStream,
        actual_wire_type: wire_format::WireType,
    ) -> error::Result<()> {
        if wire_format::WireType::WireTypeVarint == actual_wire_type {
            let v = input.read_raw_varint32()? as i32;
            self.put(Value::Enum(v));
            Ok(())
        } else {
            Err(error::Error::BadWireType {
                wire_type: actual_wire_type,
            })
        }
    }

    #[inline]
    fn merge_message(
        &mut self,
        input: &mut protobuf::CodedInputStream,
        descriptors: &descriptor::Descriptors,
        message: &descriptor::MessageDescriptor,
        actual_wire_type: wire_format::WireType,
    ) -> error::Result<()> {
        if wire_format::WireType::WireTypeLengthDelimited == actual_wire_type {
            let len = input.read_raw_varint64()?;
            let mut msg = match *self {
                Field::Singular(ref mut o) => {
                    if let Some(Value::Message(m)) = o.take() {
                        m
                    } else {
                        Message::new(message)
                    }
                }
                _ => Message::new(message),
            };

            let old_limit = input.push_limit(len)?;
            msg.merge_from(descriptors, message, input)?;
            input.pop_limit(old_limit);

            self.put(Value::Message(msg));
            Ok(())
        } else {
            Err(error::Error::BadWireType {
                wire_type: actual_wire_type,
            })
        }
    }

    #[inline]
    fn put(&mut self, value: Value) {
        match *self {
            Field::Singular(ref mut s) => *s = Some(value),
            Field::Repeated(ref mut r) => r.push(value),
        }
    }
}
