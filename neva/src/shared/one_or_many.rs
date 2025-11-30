//! Type representing either a vector or a single value if `T`

use std::{ops::{Deref, DerefMut}};
use serde::{Serialize, Deserialize};

/// Type representing either a vector or a single value if `T`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    /// Represents a single value.
    One(T),
    
    /// Represents a vector of values.
    Many(Vec<T>)
}

impl<T> From<T> for OneOrMany<T> {
    #[inline]
    fn from(v: T) -> Self {
        Self::One(v)
    }
}

impl<T> From<Vec<T>> for OneOrMany<T> {
    #[inline]
    fn from(v: Vec<T>) -> Self {
        if v.len() == 1 {
            Self::One(v.into_iter()
                .next()
                .expect("Expected at least one element in vector, but got an empty vector."))
        } else {
            Self::Many(v)
        }
    }
}

impl<T> IntoIterator for OneOrMany<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            OneOrMany::One(v) => vec![v].into_iter(),
            OneOrMany::Many(v) => v.into_iter(),
        }
    }
}


impl<T> Deref for OneOrMany<T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<T> DerefMut for OneOrMany<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_slice_mut()
    }
}

impl<T> Default for OneOrMany<T> {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
} 

impl<T> OneOrMany<T> {
    /// Creates an empty [`OneOrMany`].
    /// 
    /// Hold the [`OneOrMany::Many`] with empty vector.
    #[inline]
    pub fn new() -> Self {
        Self::Many(Vec::new())
    }
    
    /// Returns a slice of the underlying data.
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        match self {
            Self::One(t) => std::slice::from_ref(t),
            Self::Many(v) => v.as_slice(),
        }
    }

    /// Returns a mutable slice of the underlying data.
    #[inline]
    pub fn as_slice_mut(&mut self) -> &mut [T] {
        match self {
            Self::One(t) => std::slice::from_mut(t),
            Self::Many(v) => v,
        }
    }

    /// Returns a reference to underlying data if it's one element.
    /// Otherwise, returns `None`.
    #[inline]
    pub fn as_one(&self) -> Option<&T> {
        match self {
            Self::One(t) => Some(t),
            Self::Many(_) => None,
        }
    }

    /// Returns a mutable reference to underlying data if it's one element.
    /// Otherwise, returns `None`.
    #[inline]
    pub fn as_one_mut(&mut self) -> Option<&mut T> {
        match self {
            Self::One(t) => Some(t),
            Self::Many(_) => None,
        }
    }

    /// Converts `OneOrMany` into a vector.
    #[inline]
    pub fn into_vec(self) -> Vec<T> {
        match self {
            Self::One(v) => vec![v],
            Self::Many(v) => v,
        }
    }

    /// Returns the number of elements also referred to as its 'length'.
    #[inline]
    pub fn len(&self) -> usize {
        match self {
            Self::One(_) => 1,
            Self::Many(v) => v.len(),
        }
    }

    /// Returns `true` if the vector contains no elements.
    /// Otherwise, returns `false`.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        match self { 
            Self::One(_) => false,
            Self::Many(v) => v.is_empty(),
        }
    }

    /// Appends a value onto the back of a collection.
    /// 
    /// If it's called on [`OneOrMany::One`],
    /// it became [`OneOrMany::Many`] with the old value in the front.
    #[inline]
    pub fn push(&mut self, value: T) {
        match self {
            OneOrMany::One(_) => {
                let old = match std::mem::replace(self, OneOrMany::Many(Vec::new())) {
                    OneOrMany::One(v) => v,
                    OneOrMany::Many(_) => unreachable!(),
                };
                if let OneOrMany::Many(vec) = self {
                    vec.push(old);
                    vec.push(value);
                }
            },
            OneOrMany::Many(vec) => {
                vec.push(value);
                match vec.len() {
                    0 => {}, // leave Many([])
                    1 => {
                        let only = vec.pop().unwrap();
                        *self = OneOrMany::One(only);
                    },
                    _ => {}
                }
            }
        }
    }

    /// Removes the last element from a vector and returns it, or None if it is empty.
    ///
    /// If it's called on [`OneOrMany::One`],
    /// it became [`OneOrMany::Many`] with an empty vector.
    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        match self {
            OneOrMany::One(_) => {
                if let OneOrMany::One(v) = std::mem::replace(self, OneOrMany::Many(Vec::new())) {
                    return Some(v);
                }
                unreachable!()
            }
            OneOrMany::Many(vec) => {
                let value = vec.pop();
                match vec.len() {
                    0 => {}, // leave Many([])
                    1 => {
                        let only = vec.pop().unwrap();
                        *self = OneOrMany::One(only);
                    },
                    _ => {}
                }
                value
            }
        }
    }

    /// Removes and returns the element at position `index` within the vector, 
    /// shifting all elements after it to the left.
    ///
    /// If it's called on [`OneOrMany::One`],
    /// it became [`OneOrMany::Many`] with an empty vector.
    /// 
    /// # Panics
    /// Panics if index is out of bounds.
    #[inline]
    pub fn remove(&mut self, index: usize) -> T {
        match self {
            OneOrMany::One(_) => {
                assert!(index < 1, "Index out of bounds");
                
                if let OneOrMany::One(v) = std::mem::replace(self, OneOrMany::Many(Vec::new())) {
                    v
                } else {
                    unreachable!()
                }
            },
            OneOrMany::Many(vec) => {
                let value = vec.remove(index);
                match vec.len() {
                    0 => {}, // leave Many([])
                    1 => {
                        let only = vec.pop().unwrap();
                        *self = OneOrMany::One(only);
                    },
                    _ => {}
                }

                value
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_creates_new() {
        let empty = OneOrMany::<i32>::new();
        assert!(matches!(empty, OneOrMany::Many(_)));
        assert_eq!(empty.len(), 0);
    }   
    
    #[test]
    fn it_can_be_created_from_single_value() {
        let one = OneOrMany::from(1);
        match one {
            OneOrMany::One(v) => assert_eq!(v, 1),
            _ => panic!("Expected One"),
        }
    }

    #[test]
    fn it_can_be_created_from_vec_with_multiple_values() {
        let many = OneOrMany::<i32>::from(vec![1, 2]);
        match many {
            OneOrMany::Many(v) => assert_eq!(v, vec![1, 2]),
            _ => panic!("Expected Many"),
        }
    }

    #[test]
    fn it_can_be_created_from_vec_with_single_value() {
        let one = OneOrMany::<i32>::from(vec![1]);
        match one {
            OneOrMany::One(v) => assert_eq!(v, 1),
            _ => panic!("Expected One, because vec had only 1 element"),
        }
    }

    #[test]
    fn it_can_be_created_from_empty_vec() {
        let many: OneOrMany<i32> = OneOrMany::from(vec![]);
        assert!(matches!(many, OneOrMany::Many(_)));
        assert_eq!(many.len(), 0);
    }

    #[test]
    fn it_can_be_iterated() {
        let one = OneOrMany::from(1);
        let many = OneOrMany::from(vec![2, 3]);

        let mut iter_one = one.into_iter();
        assert_eq!(iter_one.next(), Some(1));
        assert_eq!(iter_one.next(), None);

        let mut iter_many = many.into_iter();
        assert_eq!(iter_many.next(), Some(2));
        assert_eq!(iter_many.next(), Some(3));
        assert_eq!(iter_many.next(), None);
    }

    #[test]
    fn it_can_be_dereferenced_as_slice() {
        let one = OneOrMany::from(1);
        assert_eq!(&*one, &[1]);

        let many = OneOrMany::<i32>::from(vec![1, 2]);
        assert_eq!(&*many, &[1, 2]);
    }

    #[test]
    fn it_can_be_dereferenced_mutably() {
        let mut one = OneOrMany::from(1);
        one[0] = 2;
        assert_eq!(one.as_one(), Some(&2));

        let mut many = OneOrMany::from(vec![1, 2]);
        many[0] = 3;
        assert_eq!(many.as_slice(), &[3, 2]);
    }

    #[test]
    fn it_can_return_as_one() {
        let one = OneOrMany::from(1);
        assert_eq!(one.as_one(), Some(&1));

        let many = OneOrMany::<i32>::from(vec![1, 2]);
        assert_eq!(many.as_one(), None);
    }

    #[test]
    fn it_can_return_as_one_mut() {
        let mut one = OneOrMany::from(1);
        if let Some(val) = one.as_one_mut() {
            *val = 2;
        }
        assert_eq!(one.as_one(), Some(&2));

        let mut many = OneOrMany::<i32>::from(vec![1, 2]);
        assert_eq!(many.as_one_mut(), None);
    }

    #[test]
    fn it_can_be_converted_into_vec() {
        let one = OneOrMany::from(1);
        assert_eq!(one.into_vec(), vec![1]);

        let many = OneOrMany::<i32>::from(vec![1, 2]);
        assert_eq!(many.into_vec(), vec![1, 2]);
    }

    #[test]
    fn it_returns_correct_length() {
        let one = OneOrMany::from(1);
        assert_eq!(one.len(), 1);

        let many = OneOrMany::<i32>::from(vec![1, 2, 3]);
        assert_eq!(many.len(), 3);
    }

    #[test]
    fn it_returns_is_empty_correctly() {
        let one = OneOrMany::from(1);
        assert!(!one.is_empty());

        let many_empty = OneOrMany::<i32>::Many(vec![]);
        assert!(many_empty.is_empty());

        let many_non_empty = OneOrMany::<i32>::from(vec![1, 2]);
        assert!(!many_non_empty.is_empty());
    }

    #[test]
    fn it_serializes_one_correctly() {
        let one = OneOrMany::from(42);
        let json = serde_json::to_string(&one).unwrap();
        assert_eq!(json, "42");
    }

    #[test]
    fn it_serializes_many_correctly() {
        let many = OneOrMany::<i32>::from(vec![1, 2, 3]);
        let json = serde_json::to_string(&many).unwrap();
        assert_eq!(json, "[1,2,3]");
    }

    #[test]
    fn it_deserializes_one_correctly() {
        let json = "42";
        let one: OneOrMany<i32> = serde_json::from_str(json).unwrap();
        match one {
            OneOrMany::One(v) => assert_eq!(v, 42),
            _ => panic!("Expected One"),
        }
    }

    #[test]
    fn it_deserializes_many_correctly() {
        let json = "[1, 2, 3]";
        let many: OneOrMany<i32> = serde_json::from_str(json).unwrap();
        match many {
            OneOrMany::Many(v) => assert_eq!(v, vec![1, 2, 3]),
            _ => panic!("Expected Many"),
        }
    }
    
    #[test]
    fn it_pushes_to_one() {
        let mut one = OneOrMany::from(1);
        one.push(2);
        
        assert!(matches!(one, OneOrMany::Many(_)));
        assert_eq!(one.as_slice(), &[1, 2]);
    }

    #[test]
    fn it_pushes_to_many() {
        let mut one = OneOrMany::from(vec![1, 2, 3]);
        one.push(4);

        assert!(matches!(one, OneOrMany::Many(_)));
        assert_eq!(one.as_slice(), &[1, 2, 3, 4]);
    }
    
    #[test]
    fn it_pushes_to_many_with_empty_one() {
        let mut one = OneOrMany::<i32>::from(vec![]);
        one.push(1);
        
        assert!(matches!(one, OneOrMany::One(_)));
        assert_eq!(one.as_one(), Some(&1));
    }
    
    #[test]
    fn it_pops_from_many() {
        let mut many = OneOrMany::from(vec![1, 2]);
        assert_eq!(many.pop(), Some(2));
        
        assert!(matches!(many, OneOrMany::One(_)));
        assert_eq!(many.as_one(), Some(&1));
    }

    #[test]
    fn it_pops_from_one() {
        let mut many = OneOrMany::from(1);
        assert_eq!(many.pop(), Some(1));

        assert!(matches!(many, OneOrMany::Many(_)));
        assert_eq!(many.len(), 0);
    }
    
    #[test]
    fn it_removes_from_many() {
        let mut many = OneOrMany::<i32>::from(vec![1, 2]);
        assert_eq!(many.remove(0), 1);
        
        assert!(matches!(many, OneOrMany::One(_)));
        assert_eq!(many.as_one(), Some(&2));
    }
    
    #[test]
    fn it_removes_from_one() {
        let mut one = OneOrMany::from(1);
        assert_eq!(one.remove(0), 1);
        
        assert!(matches!(one, OneOrMany::Many(_)));
        assert_eq!(one.len(), 0);
    }
    
    #[test]
    fn it_can_be_indexed() {
        let one = OneOrMany::<i32>::from(vec![1, 2, 3]);
        assert_eq!(one[1], 2);
    }
    
    #[test]
    fn it_can_be_indexed_mutably() {
        let mut one = OneOrMany::<i32>::from(vec![1, 2, 3]);
        one[1] = 4;
        assert_eq!(one.as_slice(), &[1, 4, 3]);
    }
}