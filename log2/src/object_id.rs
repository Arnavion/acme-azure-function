#[derive(Clone, Copy)]
pub struct ObjectId<T>(pub T);

impl std::fmt::Display for ObjectId<&'_ str> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(self.0)?;
		Ok(())
	}
}

impl<'a, T> From<&'a T> for ObjectId<&'a str>
where
	T: AsRef<str> + ?Sized,
{
	fn from(s: &'a T) -> Self {
		ObjectId(s.as_ref())
	}
}

impl std::fmt::Display for ObjectId<(&'_ str, &'_ str)> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(self.0.0)?;
		f.write_str("/")?;
		f.write_str(self.0.1)?;
		Ok(())
	}
}

impl<'a, T1, T2> From<(&'a T1, &'a T2)> for ObjectId<(&'a str, &'a str)>
where
	T1: AsRef<str> + ?Sized,
	T2: AsRef<str> + ?Sized,
{
	fn from((s1, s2): (&'a T1, &'a T2)) -> Self {
		ObjectId((s1.as_ref(), s2.as_ref()))
	}
}

impl std::fmt::Display for ObjectId<(&'_ str, &'_ str, &'_ str)> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(self.0.0)?;
		f.write_str("/")?;
		f.write_str(self.0.1)?;
		f.write_str("/")?;
		f.write_str(self.0.2)?;
		Ok(())
	}
}

impl<'a, T1, T2, T3> From<(&'a T1, &'a T2, &'a T3)> for ObjectId<(&'a str, &'a str, &'a str)>
where
	T1: AsRef<str> + ?Sized,
	T2: AsRef<str> + ?Sized,
	T3: AsRef<str> + ?Sized,
{
	fn from((s1, s2, s3): (&'a T1, &'a T2, &'a T3)) -> Self {
		ObjectId((s1.as_ref(), s2.as_ref(), s3.as_ref()))
	}
}

impl std::fmt::Display for ObjectId<&'_ hyper::Uri> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}", self.0)
	}
}

impl<'a> From<&'a hyper::Uri> for ObjectId<&'a hyper::Uri> {
	fn from(uri: &'a hyper::Uri) -> Self {
		ObjectId(uri)
	}
}

impl<T> serde::Serialize for ObjectId<T> where ObjectId<T>: std::fmt::Display {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {
		serializer.collect_str(self)
	}
}
