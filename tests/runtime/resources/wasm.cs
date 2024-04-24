using System.Diagnostics;
using ResourcesWorld;
using ResourcesWorld.wit.imports;

namespace ResourcesWorld.wit.exports
{
    public class ExportsImpl : IExports
    {
        public static IExports.Z Add(IExports.Z a, IExports.Z b)
        {
            var myA = (Z) RepTable.Get((int) a.handle);
            var myB = (Z) RepTable.Get((int) b.handle);
            return new Z(myA.val + myB.val);
        }
        
        public static Result<None, string> TestImports()
        {
            var y = new IImports.Y(10);
            Debug.Assert(y.GetA() == 10);

            // TODO

            return Result<None, string>.ok(new None());
        }

        public class X : IExports.X {
            public int val;

            public X(int val) {
                this.val = val;
            }

            override public void SetA(int val) {
                this.val = val;
            }

            override public int GetA() {
                return val;
            }

            public static X Add(IExports.X a, int b) {
                var myA = (X) RepTable.Get((int) a.handle);
                myA.SetA(myA.GetA() + b);
                return myA;
            }
        }
    
        public class Z : IExports.Z {
            private static int numDropped = 0;
            
            public int val;

            public Z(int val) {
                this.val = val;
            }

            override public int GetA() {
                return val;
            }

            public static int NumDropped() {
                return numDropped + 1;
            }

            override protected void Dispose(bool disposing) {
                if (handle.HasValue) {
                    numDropped += 1;
                }
                
                base.Dispose(disposing);
            }
        }

        public class KebabCase : IExports.KebabCase {
            public uint val;
            
            public KebabCase(uint val) {
                this.val = val;
            }
            
            override public uint GetA() {
                return val;
            }

            public static uint TakeOwned(IExports.KebabCase a) {
                var myA = (KebabCase) RepTable.Get((int) a.handle);
                return myA.val;
            }
        }
    }
}
